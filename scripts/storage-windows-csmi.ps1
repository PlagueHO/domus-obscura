[CmdletBinding()]
param(
    [string] $EvidencePath,
    [switch] $ParserOnly,
    [switch] $RaidConfiguration,
    [string] $ExpectedEvidencePath
)

$ErrorActionPreference = 'Stop'
if ('StorageWindowsCsmi' -as [type]) { throw 'Use a fresh PowerShell process.' }
$sources = @('storage-windows-topology.cs', 'storage-windows-csmi.cs', 'storage-windows-csmi-raid.cs')
if (-not $ParserOnly) {
    if (-not $EvidencePath) { throw 'A new private evidence path is required.' }
    $destination = [IO.Path]::GetFullPath($EvidencePath)
    $repository = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot)).TrimEnd('\') + '\'
    if ($destination.StartsWith($repository, [StringComparison]::OrdinalIgnoreCase) -or
        -not [IO.Directory]::Exists([IO.Path]::GetDirectoryName($destination)) -or
        [IO.File]::Exists($destination) -or [IO.Directory]::Exists($destination)) {
        throw 'Use a new private evidence file outside the repository.'
    }
    $sources += 'storage-windows-prototype.cs', 'storage-windows-backing-metadata.cs'
}
$paths = @($sources | ForEach-Object { Join-Path $PSScriptRoot $_ })
Add-Type -Path $paths

if ($ParserOnly) {
    foreach ($command in 1, 10, 20) {
        $frame = [StorageWindowsCsmi]::Frame($command)
        if ([BitConverter]::ToUInt32($frame, 0) -ne 28 -or
            [BitConverter]::ToUInt32($frame, 20) -ne [uint32]::MaxValue -or
            [BitConverter]::ToUInt32($frame, 24) -ne $frame.Length - 28) {
            throw 'Generated CSMI metadata request is malformed.'
        }
        [BitConverter]::GetBytes([uint32]0).CopyTo($frame, 20)
        if ([StorageWindowsCsmi]::Decode($command, $frame, $frame.Length) -ne 0) {
            throw 'Generated CSMI response did not decode.'
        }
        [BitConverter]::GetBytes([uint32]1).CopyTo($frame, 20)
        if ([StorageWindowsCsmi]::Decode($command, $frame, $frame.Length) -ne 1) {
            throw 'Driver refusal was hidden.'
        }
    }
    $refusals = 0
    foreach ($command in 0, 2, 4, 11, 21, 25, 29, 60, [uint32]::MaxValue) {
        try { [StorageWindowsCsmi]::Frame($command) | Out-Null }
        catch [System.Management.Automation.MethodInvocationException] {
            if ($_.Exception.InnerException -isnot [IO.IOException]) { throw }
            $refusals++
        }
    }
    foreach ($case in @(
        @{ Command = 1; Offset = 0; Value = 27 },
        @{ Command = 1; Offset = 4; Value = 0 },
        @{ Command = 1; Offset = 16; Value = 10 },
        @{ Command = 1; Offset = 24; Value = 175 },
        @{ Command = 1; Offset = 20; Value = [uint32]::MaxValue },
        @{ Command = 10; Offset = 28; Value = 9 },
        @{ Command = 20; Offset = 28; Value = 7 },
        @{ Command = 20; Offset = 100; Value = 1 }
    )) {
        $frame = [StorageWindowsCsmi]::Frame($case.Command)
        [BitConverter]::GetBytes([uint32]0).CopyTo($frame, 20)
        [BitConverter]::GetBytes([uint32]$case.Value).CopyTo($frame, $case.Offset)
        try { [StorageWindowsCsmi]::Decode($case.Command, $frame, $frame.Length) | Out-Null }
        catch [System.Management.Automation.MethodInvocationException] {
            if ($_.Exception.InnerException -isnot [IO.IOException]) { throw }
            $refusals++
        }
    }
    $frame = [StorageWindowsCsmi]::Frame(20)
    [BitConverter]::GetBytes([uint32]0).CopyTo($frame, 20)
    $frame[28] = 6
    $frame[32 + 5 * 64] = 16
    if ([StorageWindowsCsmi]::Decode(20, $frame, $frame.Length) -ne 0) {
        throw 'Exact six-PHY record budget failed.'
    }
    try { [StorageWindowsCsmi]::Decode(20, $frame, 2079) | Out-Null }
    catch [System.Management.Automation.MethodInvocationException] {
        if ($_.Exception.InnerException -isnot [IO.IOException]) { throw }
        $refusals++
    }
    $frame = [StorageWindowsCsmi]::Frame(1)
    [BitConverter]::GetBytes([uint32]0).CopyTo($frame, 20)
    for ($i = 28; $i -lt 109; $i++) { $frame[$i] = 65 }
    try { [StorageWindowsCsmi]::Decode(1, $frame, $frame.Length) | Out-Null }
    catch [System.Management.Automation.MethodInvocationException] {
        if ($_.Exception.InnerException -isnot [IO.IOException]) { throw }
        $refusals++
    }
    if ($refusals -ne 19) { throw 'Required CSMI parser refusals did not run.' }
    $driver = [StorageWindowsCsmi]::Frame(1)
    [BitConverter]::GetBytes([uint32]0).CopyTo($driver, 20)
    [BitConverter]::GetBytes([uint16]81).CopyTo($driver, 200)
    $raid = [StorageWindowsCsmi]::Frame(10)
    [BitConverter]::GetBytes([uint32]0).CopyTo($raid, 20)
    [BitConverter]::GetBytes([uint32]1).CopyTo($raid, 28)
    [BitConverter]::GetBytes([uint32]3).CopyTo($raid, 32)
    if ([StorageWindowsCsmi]::RaidPrerequisites($driver, $raid, $driver, $raid) -ne 3) {
        throw 'Generated approved RAID prerequisites failed.'
    }
    foreach ($capacity in 1, 3, 6) {
        $frame = [StorageWindowsCsmi]::RaidFrame($capacity)
        if ($frame.Length -ne 64 + $capacity * 128 -or
            [BitConverter]::ToUInt32($frame, 16) -ne 11 -or
            [BitConverter]::ToUInt32($frame, 20) -ne [uint32]::MaxValue -or
            [BitConverter]::ToUInt32($frame, 28) -ne 0) {
            throw 'Dedicated bounded set-zero request failed.'
        }
        $boundedRaid = $raid.Clone()
        [BitConverter]::GetBytes([uint32]$capacity).CopyTo($boundedRaid, 32)
        if ([StorageWindowsCsmi]::RaidPrerequisites($driver, $boundedRaid, $driver, $boundedRaid) -ne $capacity) {
            throw 'Exact reported member-capacity boundary failed.'
        }
    }
    $raidRefusals = 0
    foreach ($capacity in 0, 7) {
        try { [StorageWindowsCsmi]::RaidFrame($capacity) | Out-Null }
        catch [System.Management.Automation.MethodInvocationException] {
            if ($_.Exception.InnerException -isnot [IO.IOException]) { throw }
            $raidRefusals++
        }
    }
    foreach ($kind in 'driver-version', 'sets', 'max-zero', 'max-seven', 'driver-changed',
        'raid-changed', 'driver-refused', 'raid-refused') {
        $changedDriver = $driver.Clone()
        $changedRaid = $raid.Clone()
        switch ($kind) {
            'driver-version' { $changedDriver[200] = 90 }
            'sets' { $changedRaid[28] = 2 }
            'max-zero' { $changedRaid[32] = 0 }
            'max-seven' { $changedRaid[32] = 7 }
            'driver-changed' { $changedDriver[28] = 65 }
            'raid-changed' { $changedRaid[40] = 1 }
            'driver-refused' { $changedDriver[20] = 1 }
            'raid-refused' { $changedRaid[20] = 1 }
        }
        $comparisonDriver = $changedDriver
        $comparisonRaid = $changedRaid
        if ($kind -eq 'driver-changed') { $comparisonDriver = $driver }
        if ($kind -eq 'raid-changed') { $comparisonRaid = $raid }
        try { [StorageWindowsCsmi]::RaidPrerequisites($changedDriver, $changedRaid, $comparisonDriver, $comparisonRaid) | Out-Null }
        catch [System.Management.Automation.MethodInvocationException] {
            if ($_.Exception.InnerException -isnot [IO.IOException]) { throw }
            $raidRefusals++
        }
    }
    if ($raidRefusals -ne 10) { throw 'Required RAID prerequisite refusals did not run.' }
    '{"result":"csmi_parser_passed","refusals":19,"raid_prerequisite_refusals":10,"metadata_commands":3,"maximum_phy_records":6,"native_collection":false}'
    return
}

$expected = $null
if ($RaidConfiguration) {
    if (-not $ExpectedEvidencePath) { throw 'Prior private controller evidence is required.' }
    $expected = Get-Content -LiteralPath $ExpectedEvidencePath -Raw | ConvertFrom-Json
    if ($expected.csmi.Status -ne 'metadata_only_completed' -or
        $expected.binding_admitted -ne $false -or -not $expected.observation_rechecked) {
        throw 'Prior controller evidence is not the approved completed observation.'
    }
    $driverCommands = @($expected.csmi.Commands | Where-Object Command -eq 1)
    $raidCommands = @($expected.csmi.Commands | Where-Object Command -eq 10)
    if ($driverCommands.Count -ne 1 -or $raidCommands.Count -ne 1) {
        throw 'Prior driver and RAID metadata must be unique.'
    }
    $expectedDriver = [Convert]::FromBase64String($driverCommands[0].BytesBase64)
    $expectedRaid = [Convert]::FromBase64String($raidCommands[0].BytesBase64)
    [StorageWindowsCsmi]::RaidPrerequisites($expectedDriver, $expectedRaid,
        $expectedDriver, $expectedRaid) | Out-Null
}
$first = [StorageWindowsBackingMetadata]::Collect()
if ($first.Devices.Count -ne 1 -or -not $first.Devices[0].NumberCorrelated) {
    throw 'One correlated C: device is required.'
}
$disk = $first.Devices[0].Disk
$properties = @($first.Devices[0].Properties | Where-Object Property -eq 2)
if ($properties.Count -ne 1 -or $properties[0].Status -ne 'observed') {
    throw 'C: device-ID metadata is unavailable.'
}
$seeds = @(Get-CimInstance -Query "SELECT Index, PNPDeviceID FROM Win32_DiskDrive WHERE Index = $disk" -OperationTimeoutSec 10)
if ($seeds.Count -ne 1 -or $seeds[0].Index -ne $disk -or
    [string]::IsNullOrEmpty($seeds[0].PNPDeviceID) -or
    [Text.Encoding]::Unicode.GetByteCount($seeds[0].PNPDeviceID) -ge 65536) {
    throw 'An exact bounded C: seed is unavailable.'
}
$descriptor = [Convert]::FromBase64String($properties[0].BytesBase64)
$topology = [StorageWindowsTopology]::Collect($seeds[0].PNPDeviceID, $disk, $descriptor)
if ($topology.StopReason -ne 'controller_observed' -or $topology.Nodes.Count -ne 2 -or
    $topology.Nodes[1].Class.Value -ne 'SCSIAdapter' -or
    $topology.Nodes[1].Service.Status -ne 0 -or
    $topology.Nodes[1].Service.Value -ne 'iaStorAVC') {
    throw 'The approved C: Intel controller profile is not present.'
}
if ($RaidConfiguration) {
    if (($topology | ConvertTo-Json -Depth 8 -Compress) -ne
        ($expected.topology | ConvertTo-Json -Depth 8 -Compress) -or
        $first.Root.Identity -ne $expected.native.Root.Identity -or
        $first.Root.Coordinate -ne $expected.native.Root.Coordinate -or
        ($first.Extents | ConvertTo-Json -Compress) -ne
        ($expected.native.Extents | ConvertTo-Json -Compress) -or
        ($first.Devices | ConvertTo-Json -Depth 6 -Compress) -ne
        ($expected.native.Devices | ConvertTo-Json -Depth 6 -Compress)) {
        throw 'C: or its controller changed from the approved private observation.'
    }
    $probe = [StorageWindowsCsmi]::CollectRaid($topology.Nodes[0].InstanceId,
        $topology.Nodes[1].InstanceId, $disk, $descriptor, $expectedDriver, $expectedRaid)
}
else {
    $probe = [StorageWindowsCsmi]::Collect($topology.Nodes[0].InstanceId,
        $topology.Nodes[1].InstanceId, $disk, $descriptor)
}
$recheck = [StorageWindowsTopology]::Collect($seeds[0].PNPDeviceID, $disk, $descriptor)
$last = [StorageWindowsBackingMetadata]::Collect()
if (($topology | ConvertTo-Json -Depth 8 -Compress) -ne
    ($recheck | ConvertTo-Json -Depth 8 -Compress) -or
    $first.Root.Identity -ne $last.Root.Identity -or
    $first.Root.Coordinate -ne $last.Root.Coordinate -or
    ($first.Extents | ConvertTo-Json -Compress) -ne ($last.Extents | ConvertTo-Json -Compress) -or
    ($first.Devices | ConvertTo-Json -Depth 6 -Compress) -ne ($last.Devices | ConvertTo-Json -Depth 6 -Compress)) {
    throw 'C: controller/storage observations changed around the metadata probe.'
}
$result = [ordered]@{
    run = [Guid]::NewGuid().ToString('N')
    scope = if ($RaidConfiguration) { 'approved single C: controller CSMI metadata commands 1 10 11 only; RAID set 0' }
        else { 'approved C: controller CSMI metadata commands 1 10 20 only' }
    maximum_response_bytes = 65536
    maximum_csmi_records = 8
    native = $first
    topology = $topology
    csmi = $probe
    observation_rechecked = $true
    continuous_root_lease = $false
    binding_admitted = $false
    sources = @($paths + $PSCommandPath | ForEach-Object {
        @{ name = [IO.Path]::GetFileName($_); sha256 = (Get-FileHash -LiteralPath $_).Hash }
    })
}
$stream = [IO.File]::Open($destination, [IO.FileMode]::CreateNew,
    [IO.FileAccess]::Write, [IO.FileShare]::None)
try {
    $bytes = [Text.UTF8Encoding]::new($false).GetBytes(($result | ConvertTo-Json -Depth 14) + "`n")
    $stream.Write($bytes, 0, $bytes.Length)
}
finally { $stream.Dispose() }
[ordered]@{
    run = $result.run
    result = if ($probe.Status -in 'metadata_only_completed', 'raid_candidate_completed') { 'metadata_probe_completed' } else { 'metadata_probe_blocked' }
    status = $probe.Status
    configuration_status = $probe.ConfigurationStatus
    open_error = $probe.OpenError
    commands_observed = @($probe.Commands | Where-Object { $null -ne $_ }).Count
    candidate_members = $probe.CandidateMembers
    controller_correlated = $probe.ControllerCorrelated
    observation_rechecked = $true
    native_handles_disposed = $true
    binding_admitted = $false
} | ConvertTo-Json

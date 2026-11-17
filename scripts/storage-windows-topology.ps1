[CmdletBinding()]
param(
    [string] $EvidencePath,
    [switch] $ParserOnly
)

$ErrorActionPreference = 'Stop'
if ($ParserOnly) {
    if ('StorageWindowsTopology' -as [type]) { throw 'Use a fresh PowerShell process.' }
    Add-Type -Path (Join-Path $PSScriptRoot 'storage-windows-topology.cs')
    $valid = [Text.Encoding]::Unicode.GetBytes("disk`0")
    if ([StorageWindowsTopology]::DecodeString(1, $valid, $valid.Length) -ne 'disk' -or
        [StorageWindowsTopology]::DecodeInterface("disk`0`0".ToCharArray()) -ne 'disk') {
        throw 'Generated topology string decoding failed.'
    }
    $boundary = [byte[]]::new(65536)
    for ($i = 0; $i -lt 65534; $i += 2) { $boundary[$i] = 65 }
    if ([StorageWindowsTopology]::DecodeString(1, $boundary, 65536).Length -ne 32767) {
        throw 'Exact 64 KiB topology property boundary failed.'
    }
    $interface = [char[]]::new(32768)
    for ($i = 0; $i -lt 32766; $i++) { $interface[$i] = 'a' }
    if ([StorageWindowsTopology]::DecodeInterface($interface).Length -ne 32766) {
        throw 'Exact 64 KiB interface boundary failed.'
    }
    $refusals = 0
    foreach ($case in @(
        @{ Type = 2; Bytes = $valid; Length = 10 },
        @{ Type = 1; Bytes = $valid; Length = 9 },
        @{ Type = 1; Bytes = $valid; Length = 12 },
        @{ Type = 1; Bytes = [byte[]]::new(65538); Length = 65538 },
        @{ Type = 1; Bytes = [Text.Encoding]::Unicode.GetBytes('disk'); Length = 8 },
        @{ Type = 1; Bytes = [Text.Encoding]::Unicode.GetBytes("di`0sk`0"); Length = 12 },
        @{ Type = 1; Bytes = [byte[]]@(0,216,0,0); Length = 4 }
    )) {
        try { [StorageWindowsTopology]::DecodeString($case.Type, $case.Bytes, $case.Length) | Out-Null }
        catch [System.Management.Automation.MethodInvocationException] {
            if ($_.Exception.InnerException -isnot [IO.IOException] -and
                $_.Exception.InnerException -isnot [Text.DecoderFallbackException]) { throw }
            $refusals++
        }
    }
    foreach ($bytes in @(
        "disk".ToCharArray(), "disk`0".ToCharArray(), "disk`0other`0`0".ToCharArray(),
        "`0`0".ToCharArray(), [char[]]::new(32769)
    )) {
        try { [StorageWindowsTopology]::DecodeInterface($bytes) | Out-Null }
        catch [System.Management.Automation.MethodInvocationException] {
            if ($_.Exception.InnerException -isnot [IO.IOException]) { throw }
            $refusals++
        }
    }
    $physical = [byte[]]::new(192)
    foreach ($entry in @(@(0,56),@(4,192),@(8,1),@(32,1),@(36,136),@(40,40))) {
        [BitConverter]::GetBytes([uint32]$entry[1]).CopyTo($physical, $entry[0])
    }
    if ([StorageWindowsTopology]::DecodePhysical($physical, 192, 8).Records -ne 2) {
        throw 'Generated physical topology shape did not decode.'
    }
    foreach ($entry in @(@(0,16),@(4,191),@(8,9),@(20,2),@(32,8),@(36,135),@(40,39),@(40,65535))) {
        $bytes = $physical.Clone()
        [BitConverter]::GetBytes([uint32]$entry[1]).CopyTo($bytes, $entry[0])
        try { [StorageWindowsTopology]::DecodePhysical($bytes, 192, 8) | Out-Null }
        catch [System.Management.Automation.MethodInvocationException] {
            if ($_.Exception.InnerException -isnot [IO.IOException]) { throw }
            $refusals++
        }
    }
    $maximum = [byte[]]::new(1000)
    foreach ($entry in @(@(0,56),@(4,1000),@(8,1),@(20,1),@(24,128),@(28,40),
        @(32,6),@(36,816),@(40,168))) {
        [BitConverter]::GetBytes([uint32]$entry[1]).CopyTo($maximum, $entry[0])
    }
    if ([StorageWindowsTopology]::DecodePhysical($maximum, 1000, 8).Records -ne 8) {
        throw 'Exact eight-record topology boundary failed.'
    }
    foreach ($offset in 0, 1) {
        $bytes = $maximum.Clone()
        if ($offset -eq 0) { [BitConverter]::GetBytes([uint32]40).CopyTo($bytes, 40) }
        $budget = if ($offset -eq 0) { 8 } else { 7 }
        try { [StorageWindowsTopology]::DecodePhysical($bytes, 1000, $budget) | Out-Null }
        catch [System.Management.Automation.MethodInvocationException] {
            if ($_.Exception.InnerException -isnot [IO.IOException]) { throw }
            $refusals++
        }
    }
    if ($refusals -ne 22) { throw 'Required topology parser refusals did not run.' }
    '{"result":"topology_parser_passed","malformed_refusals":22,"maximum_records":8,"exact_response_bound":65536,"native_collection":false}'
    return
}
if (-not $EvidencePath) { throw 'A new private evidence path is required.' }
$destination = [IO.Path]::GetFullPath($EvidencePath)
$repository = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot)).TrimEnd('\') + '\'
if ($destination.StartsWith($repository, [StringComparison]::OrdinalIgnoreCase) -or
    -not [IO.Directory]::Exists([IO.Path]::GetDirectoryName($destination)) -or
    [IO.File]::Exists($destination) -or [IO.Directory]::Exists($destination)) {
    throw 'Use a new private evidence file outside the repository.'
}
if ('StorageWindowsTopology' -as [type]) {
    throw 'Use a fresh PowerShell process for topology collection.'
}
$sources = @('storage-windows-prototype.cs', 'storage-windows-backing-metadata.cs',
    'storage-windows-topology.cs') | ForEach-Object { Join-Path $PSScriptRoot $_ }
Add-Type -Path $sources
$first = [StorageWindowsBackingMetadata]::Collect()
if ($first.Devices.Count -ne 1 -or -not $first.Devices[0].NumberCorrelated) {
    throw 'This bounded topology diagnostic requires one correlated C: device.'
}
$disk = $first.Devices[0].Disk
$properties = @($first.Devices[0].Properties | Where-Object Property -eq 2)
if ($properties.Count -ne 1 -or $properties[0].Status -ne 'observed') {
    throw 'C: device-ID metadata is unavailable.'
}
# CIM supplies only a seed. Native interface queries must corroborate it.
$seeds = @(Get-CimInstance -Query "SELECT Index, PNPDeviceID FROM Win32_DiskDrive WHERE Index = $disk" -OperationTimeoutSec 10)
if ($seeds.Count -ne 1 -or $seeds[0].Index -ne $disk -or
    [string]::IsNullOrEmpty($seeds[0].PNPDeviceID) -or
    [Text.Encoding]::Unicode.GetByteCount($seeds[0].PNPDeviceID) -ge 65536) {
    throw 'An exact bounded C: device seed is unavailable.'
}
$topology = [StorageWindowsTopology]::Collect($seeds[0].PNPDeviceID, $disk,
    [Convert]::FromBase64String($properties[0].BytesBase64))
$second = [StorageWindowsBackingMetadata]::Collect()
if ($first.Root.Identity -ne $second.Root.Identity -or
    $first.Root.Coordinate -ne $second.Root.Coordinate -or
    ($first.Extents | ConvertTo-Json -Compress) -ne ($second.Extents | ConvertTo-Json -Compress) -or
    ($first.Devices | ConvertTo-Json -Depth 6 -Compress) -ne ($second.Devices | ConvertTo-Json -Depth 6 -Compress)) {
    throw 'C: observation changed around topology collection.'
}
$result = [ordered]@{
    run = [Guid]::NewGuid().ToString('N')
    scope = 'approved read-only C: related storage topology'
    maximum_nodes = 8
    maximum_native_response_bytes = 65536
    seed_source = 'CIM exact Index query; native interface corroborated'
    continuous_root_lease = $false
    native = $first
    topology = $topology
    observation_rechecked = $true
    complete_backing_graph = $false
    binding_admitted = $false
    sources = @($sources + $PSCommandPath | ForEach-Object {
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
    result = 'private_topology_saved'
    node_records = $topology.Nodes.Count
    stop_reason = $topology.StopReason
    interface_correlated = $topology.InterfaceCorrelated
    chain_rechecked = $topology.ChainRechecked
    physical_topology_status = $topology.PhysicalTopology.Status
    physical_topology_error = $topology.PhysicalTopology.Win32Error
    observation_rechecked = $true
    complete_backing_graph = $false
    binding_admitted = $false
    native_handles_disposed = $true
} | ConvertTo-Json

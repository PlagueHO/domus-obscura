[CmdletBinding()]
param(
    [string] $MetadataEvidencePath,
    [string] $EvidencePath
)

$ErrorActionPreference = 'Stop'
$repository = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot)).TrimEnd('\') + '\'
if ($EvidencePath) {
    $destination = [IO.Path]::GetFullPath($EvidencePath)
    if ($destination.StartsWith($repository, [StringComparison]::OrdinalIgnoreCase) -or
        -not [IO.Directory]::Exists([IO.Path]::GetDirectoryName($destination)) -or
        [IO.File]::Exists($destination) -or [IO.Directory]::Exists($destination)) {
        throw 'Use a new evidence file in an existing private directory outside the repository.'
    }
}
if ('StorageWindowsDeviceId' -as [type]) {
    throw 'Use a fresh PowerShell process for the device-ID proof.'
}
Add-Type -Path (Join-Path $PSScriptRoot 'storage-windows-device-id.cs')

function New-Descriptor([int] $Count = 1, [int] $Length = 8) {
    $step = 20 + $Length
    $bytes = [byte[]]::new(12 + $Count * $step)
    [BitConverter]::GetBytes([uint32]16).CopyTo($bytes, 0)
    [BitConverter]::GetBytes([uint32]$bytes.Length).CopyTo($bytes, 4)
    [BitConverter]::GetBytes([uint32]$Count).CopyTo($bytes, 8)
    for ($i = 0; $i -lt $Count; $i++) {
        $offset = 12 + $i * $step
        [BitConverter]::GetBytes([uint32]1).CopyTo($bytes, $offset)
        [BitConverter]::GetBytes([uint32]2).CopyTo($bytes, $offset + 4)
        [BitConverter]::GetBytes([uint16]$Length).CopyTo($bytes, $offset + 8)
        [BitConverter]::GetBytes([uint16]$step).CopyTo($bytes, $offset + 10)
        $bytes[$offset + 16 + $Length - 1] = [byte]($i + 1)
    }
    return ,$bytes
}

function Assert-Refusal([byte[]] $Bytes, [type] $Expected) {
    try { [StorageWindowsDeviceId]::SupportedCount($Bytes) | Out-Null }
    catch [System.Management.Automation.MethodInvocationException] {
        if ($_.Exception.InnerException -isnot $Expected) { throw }
        return
    }
    throw 'Required descriptor refusal did not occur.'
}

$valid = New-Descriptor
if (-not [StorageWindowsDeviceId]::Matches($valid, $valid)) {
    throw 'Exact supported identifier did not match.'
}
$changed = $valid.Clone()
$changed[35] = 2
if ([StorageWindowsDeviceId]::Matches($valid, $changed)) {
    throw 'A different supported identifier matched.'
}
$long = New-Descriptor 1 16
[Array]::Copy($valid, 28, $long, 28, 8)
$longChanged = $long.Clone()
$longChanged[43] = 2
if (-not [StorageWindowsDeviceId]::Matches($long, $long) -or
    [StorageWindowsDeviceId]::Matches($long, $longChanged) -or
    [StorageWindowsDeviceId]::Matches($valid, $long)) {
    throw 'Sixteen-byte identifiers did not preserve their exact value and width.'
}
$pair = New-Descriptor 2
$reordered = $pair.Clone()
[Array]::Copy($pair, 28, $reordered, 56, 8)
[Array]::Copy($pair, 56, $reordered, 28, 8)
if (-not [StorageWindowsDeviceId]::Matches($pair, $reordered) -or
    [StorageWindowsDeviceId]::Matches($valid, $pair)) {
    throw 'Identifier inventory matching failed.'
}
$maximum = New-Descriptor 8
if ([StorageWindowsDeviceId]::SupportedCount($maximum) -ne 8) {
    throw 'The exact eight-identifier boundary failed.'
}
$weak = $pair.Clone()
[BitConverter]::GetBytes([uint32]0).CopyTo($weak, 44)
$weakChanged = $weak.Clone()
$weakChanged[63] = 99
if (-not [StorageWindowsDeviceId]::Matches($valid, $weak) -or
    -not [StorageWindowsDeviceId]::Matches($weak, $weakChanged)) {
    throw 'Weak metadata altered the supported identifier inventory.'
}
$refusals = 0
foreach ($case in @(
    @{ Offset = 4; Value = [uint32]39; Error = [IO.IOException] },
    @{ Offset = 8; Value = [uint32]0; Error = [IO.IOException] },
    @{ Offset = 8; Value = [uint32]9; Error = [IO.IOException] },
    @{ Offset = 0; Value = [uint32]20; Error = [NotSupportedException] },
    @{ Offset = 12; Value = [uint32]2; Error = [NotSupportedException] },
    @{ Offset = 12; Value = [uint32]4; Error = [NotSupportedException] },
    @{ Offset = 16; Value = [uint32]0; Error = [NotSupportedException] },
    @{ Offset = 16; Value = [uint32]9; Error = [NotSupportedException] },
    @{ Offset = 24; Value = [uint32]3; Error = [NotSupportedException] },
    @{ Offset = 24; Value = [uint32]1; Error = [NotSupportedException] }
)) {
    $bytes = $valid.Clone()
    [BitConverter]::GetBytes($case.Value).CopyTo($bytes, $case.Offset)
    Assert-Refusal $bytes $case.Error
    $refusals++
}
foreach ($length in 0, 9, [uint16]::MaxValue) {
    $bytes = $valid.Clone()
    [BitConverter]::GetBytes([uint16]$length).CopyTo($bytes, 20)
    Assert-Refusal $bytes $(if ($length -eq 9) { [NotSupportedException] } else { [IO.IOException] })
    $refusals++
}
foreach ($next in 0, 16, 29, [uint16]::MaxValue) {
    $bytes = $pair.Clone()
    [BitConverter]::GetBytes([uint16]$next).CopyTo($bytes, 22)
    Assert-Refusal $bytes ([IO.IOException])
    $refusals++
}
$bytes = $pair.Clone()
[Array]::Copy($pair, 28, $bytes, 56, 8)
Assert-Refusal $bytes ([IO.IOException])
$refusals++
foreach ($value in 0, 255) {
    $bytes = $valid.Clone()
    for ($i = 28; $i -lt 36; $i++) { $bytes[$i] = [byte]$value }
    Assert-Refusal $bytes ([NotSupportedException])
    $refusals++
}
Assert-Refusal (New-Descriptor 1 12) ([NotSupportedException])
$refusals++
foreach ($value in 0, 255) {
    $bytes = $long.Clone()
    for ($i = 28; $i -lt 44; $i++) { $bytes[$i] = [byte]$value }
    Assert-Refusal $bytes ([NotSupportedException])
    $refusals++
}
Assert-Refusal ([byte[]]::new(11)) ([IO.IOException])
$refusals++
Assert-Refusal ([byte[]]::new(65537)) ([IO.IOException])
$refusals++
$bytes = $valid[0..26]
[BitConverter]::GetBytes([uint32]27).CopyTo($bytes, 4)
Assert-Refusal $bytes ([IO.IOException])
$refusals++
$bytes = $valid.Clone()
[BitConverter]::GetBytes([uint16]1).CopyTo($bytes, 22)
Assert-Refusal $bytes ([IO.IOException])
$refusals++
$bytes = $weak.Clone()
[BitConverter]::GetBytes([uint32]0).CopyTo($bytes, 16)
Assert-Refusal $bytes ([NotSupportedException])
$refusals++
$bytes = $weak.Clone()
[BitConverter]::GetBytes([uint32]9).CopyTo($bytes, 44)
Assert-Refusal $bytes ([NotSupportedException])
$refusals++
$boundary = [byte[]]::new(65536)
[BitConverter]::GetBytes([uint32]16).CopyTo($boundary, 0)
[BitConverter]::GetBytes([uint32]65536).CopyTo($boundary, 4)
[BitConverter]::GetBytes([uint32]1).CopyTo($boundary, 8)
[BitConverter]::GetBytes([uint32]1).CopyTo($boundary, 12)
[BitConverter]::GetBytes([uint16]65508).CopyTo($boundary, 20)
Assert-Refusal $boundary ([NotSupportedException])
$refusals++
if ($refusals -ne 30) { throw 'Required descriptor refusals did not run.' }

$nativeStatus = 'not examined'
$sourceRun = $null
if ($MetadataEvidencePath) {
    $path = [IO.Path]::GetFullPath($MetadataEvidencePath)
    if ($path.StartsWith($repository, [StringComparison]::OrdinalIgnoreCase)) {
        throw 'Hardware evidence must remain outside the repository.'
    }
    $stream = [IO.File]::OpenRead($path)
    try {
        if ($stream.Length -gt 1048576) { throw 'Private metadata exceeds the offline proof bound.' }
        $reader = [IO.StreamReader]::new($stream)
        try { $record = $reader.ReadToEnd() | ConvertFrom-Json }
        finally { $reader.Dispose() }
    }
    finally { $stream.Dispose() }
    if ($record.scope -ne 'approved read-only C: metadata' -or
        -not $record.native.SnapshotRechecked -or $record.native.BindingAdmitted -or
        $record.native.Devices.Count -ne 1 -or
        -not $record.native.Devices[0].NumberCorrelated) {
        throw 'The offline proof requires one correlated approved C: device record.'
    }
    $properties = @($record.native.Devices[0].Properties | Where-Object Property -eq 2)
    if ($properties.Count -ne 1 -or $properties[0].Status -ne 'observed') {
        throw 'The offline proof requires exactly one observed device-ID descriptor.'
    }
    $descriptor = [Convert]::FromBase64String($properties[0].BytesBase64)
    if ($descriptor.Length -ne $properties[0].ReturnedBytes) {
        throw 'Private descriptor length does not match its recorded response.'
    }
    try {
        [StorageWindowsDeviceId]::SupportedCount($descriptor) | Out-Null
        $nativeStatus = 'candidate identifier format supported - not admitted'
    }
    catch [System.Management.Automation.MethodInvocationException] {
        if ($_.Exception.InnerException -isnot [NotSupportedException]) { throw }
        $nativeStatus = 'unsupported identifier profile - not admitted'
    }
    $sourceRun = $record.run
}
$result = [ordered]@{
    run = [Guid]::NewGuid().ToString('N')
    result = 'strict_device_id_offline_proof_passed'
    exact_match = $true
    different_identifier_refused = $true
    sixteen_byte_exact_match = $true
    identifier_width_distinct = $true
    inventory_order_independent = $true
    inventory_change_refused = $true
    weak_metadata_cannot_replace_supported_identity = $true
    maximum_identifiers = 8
    exact_response_bound_parsed = $true
    malformed_or_unsupported_refusals = $refusals
    native_collection = $false
    native_status = $nativeStatus
    source_metadata_run = $sourceRun
    binding_admitted = $false
    codec_sha256 = (Get-FileHash -LiteralPath (Join-Path $PSScriptRoot 'storage-windows-device-id.cs')).Hash
    harness_sha256 = (Get-FileHash -LiteralPath $PSCommandPath).Hash
} | ConvertTo-Json
if ($EvidencePath) {
    $stream = [IO.File]::Open($destination, [IO.FileMode]::CreateNew,
        [IO.FileAccess]::Write, [IO.FileShare]::None)
    try {
        $bytes = [Text.UTF8Encoding]::new($false).GetBytes($result + "`n")
        $stream.Write($bytes, 0, $bytes.Length)
    }
    finally { $stream.Dispose() }
}
$result

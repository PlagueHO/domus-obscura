[CmdletBinding()]
param(
    [string] $EvidencePath,
    [switch] $ParserOnly
)

$ErrorActionPreference = 'Stop'
$rootSource = Join-Path $PSScriptRoot 'storage-windows-prototype.cs'
$metadataSource = Join-Path $PSScriptRoot 'storage-windows-backing-metadata.cs'
if ('StorageWindowsBackingMetadata' -as [type]) {
    throw 'Use a fresh PowerShell process so source hashes match the compiled collector.'
}
Add-Type -Path @($rootSource, $metadataSource)

if ($ParserOnly) {
    $bytes = [byte[]]::new(32)
    [BitConverter]::GetBytes([uint32]1).CopyTo($bytes, 0)
    [BitConverter]::GetBytes([uint32]7).CopyTo($bytes, 8)
    [BitConverter]::GetBytes([int64]4096).CopyTo($bytes, 16)
    [BitConverter]::GetBytes([int64]8192).CopyTo($bytes, 24)
    $extent = [StorageWindowsBackingMetadata]::DecodeExtents($bytes, 32)
    if ($extent.Count -ne 1 -or $extent[0].Disk -ne 7 -or $extent[0].Offset -ne 4096 -or
        $extent[0].Length -ne 8192) { throw 'Exact extent decoding failed.' }
    $refusals = 0
    foreach ($count in 0, 9, [uint32]::MaxValue) {
        [BitConverter]::GetBytes([uint32]$count).CopyTo($bytes, 0)
        try { [StorageWindowsBackingMetadata]::DecodeExtents($bytes, 32) | Out-Null }
        catch [System.Management.Automation.MethodInvocationException] {
            if ($_.Exception.InnerException -isnot [IO.IOException]) { throw }
            $refusals++
        }
    }
    [BitConverter]::GetBytes([uint32]1).CopyTo($bytes, 0)
    [BitConverter]::GetBytes([int64]::MaxValue).CopyTo($bytes, 16)
    try { [StorageWindowsBackingMetadata]::DecodeExtents($bytes, 32) | Out-Null }
    catch [System.Management.Automation.MethodInvocationException] {
        if ($_.Exception.InnerException -isnot [IO.IOException]) { throw }
        $refusals++
    }
    [BitConverter]::GetBytes([uint32]40).CopyTo($bytes, 4)
    try { [StorageWindowsBackingMetadata]::DecodeDescriptorSize($bytes, 32) | Out-Null }
    catch [System.Management.Automation.MethodInvocationException] {
        if ($_.Exception.InnerException -isnot [IO.IOException]) { throw }
        $refusals++
    }
    $maximum = [byte[]]::new(200)
    [BitConverter]::GetBytes([uint32]8).CopyTo($maximum, 0)
    for ($i = 0; $i -lt 8; $i++) {
        [BitConverter]::GetBytes([uint32]$i).CopyTo($maximum, 8 + $i * 24)
        [BitConverter]::GetBytes([int64]8192).CopyTo($maximum, 24 + $i * 24)
    }
    if ([StorageWindowsBackingMetadata]::DecodeExtents($maximum, 200).Count -ne 8) {
        throw 'The exact eight-device boundary was refused.'
    }
    try { [StorageWindowsBackingMetadata]::DecodeExtents($maximum, 199) | Out-Null }
    catch [System.Management.Automation.MethodInvocationException] {
        if ($_.Exception.InnerException -isnot [IO.IOException]) { throw }
        $refusals++
    }
    $descriptor = [byte[]]::new(65536)
    [BitConverter]::GetBytes([uint32]65536).CopyTo($descriptor, 4)
    if ([StorageWindowsBackingMetadata]::DecodeDescriptorSize($descriptor, 65536) -ne 65536) {
        throw 'The exact 64 KiB response boundary was refused.'
    }
    try { [StorageWindowsBackingMetadata]::DecodeDescriptorSize($descriptor, 65537) | Out-Null }
    catch [System.Management.Automation.MethodInvocationException] {
        if ($_.Exception.InnerException -isnot [IO.IOException]) { throw }
        $refusals++
    }
    if ($refusals -ne 7) { throw 'Required malformed-response refusals did not run.' }
    '{"result":"metadata_parser_passed","exact_extent":true,"maximum_devices":8,"maximum_response_bytes":65536,"malformed_refusals":7,"native_collection":false}'
    return
}

if (-not $EvidencePath) { throw 'A private session evidence path is required.' }
$destination = [IO.Path]::GetFullPath($EvidencePath)
$repository = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot)).TrimEnd('\') + '\'
if ($destination.StartsWith($repository, [StringComparison]::OrdinalIgnoreCase) -or
    -not [IO.Directory]::Exists([IO.Path]::GetDirectoryName($destination))) {
    throw 'Hardware evidence must use an existing private directory outside the repository.'
}
if ([IO.File]::Exists($destination) -or [IO.Directory]::Exists($destination)) {
    throw 'Private evidence is create-once. Use a fresh evidence filename.'
}
$result = [ordered]@{
    run = [Guid]::NewGuid().ToString('N')
    scope = 'approved read-only C: metadata'
    maximum_devices = 8
    maximum_response_bytes = 65536
    root_source_sha256 = (Get-FileHash -LiteralPath $rootSource -Algorithm SHA256).Hash
    metadata_source_sha256 = (Get-FileHash -LiteralPath $metadataSource -Algorithm SHA256).Hash
    harness_sha256 = (Get-FileHash -LiteralPath $PSCommandPath -Algorithm SHA256).Hash
    native = [StorageWindowsBackingMetadata]::Collect()
}
$stream = [IO.File]::Open($destination, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write,
    [IO.FileShare]::None)
try {
    $bytes = [Text.UTF8Encoding]::new($false).GetBytes(($result | ConvertTo-Json -Depth 12) + "`n")
    $stream.Write($bytes, 0, $bytes.Length)
}
finally { $stream.Dispose() }
[ordered]@{
    result = 'private_metadata_saved'
    run = $result.run
    device_records = $result.native.Devices.Count
    snapshot_rechecked = $result.native.SnapshotRechecked
    dependency_status = $result.native.Dependencies.Status
    dependency_error = $result.native.Dependencies.Win32Error
    binding_admitted = $false
    native_handles_closed = $true
} | ConvertTo-Json

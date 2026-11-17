[CmdletBinding()]
param(
    [string] $InputEvidencePath
)

$ErrorActionPreference = 'Stop'
Add-Type -Path (Join-Path $PSScriptRoot 'storage-windows-csmi-raid.cs')

function New-Response([int] $Count, [int] $Capacity) {
    $bytes = [byte[]]::new(64 + $Capacity * 128)
    [BitConverter]::GetBytes([uint32]28).CopyTo($bytes, 0)
    [Text.Encoding]::ASCII.GetBytes("CSMIARY`0").CopyTo($bytes, 4)
    [BitConverter]::GetBytes([uint32]11).CopyTo($bytes, 16)
    [BitConverter]::GetBytes([uint32]($bytes.Length - 28)).CopyTo($bytes, 24)
    $bytes[43] = [byte]$Count
    for ($i = 0; $i -lt $Count; $i++) {
        $position = 64 + $i * 128
        [Text.Encoding]::ASCII.GetBytes('generated model').CopyTo($bytes, $position)
        [Text.Encoding]::ASCII.GetBytes("generated serial $i").CopyTo($bytes, $position + 48)
    }
    return ,$bytes
}

$valid = New-Response 2 2
if ([StorageWindowsCsmiRaid]::Decode($valid, $valid.Length, 0, 2, 0, 81) -ne 2) {
    throw 'Exact historical two-member candidate failed.'
}
$maximum = New-Response 6 6
if ([StorageWindowsCsmiRaid]::Decode($maximum, $maximum.Length, 0, 6, 0, 81) -ne 6) {
    throw 'Exact six-member candidate boundary failed.'
}
$spare = New-Response 2 6
if ([StorageWindowsCsmiRaid]::Decode($spare, $spare.Length, 0, 6, 0, 81) -ne 2) {
    throw 'Zero unused candidate capacity failed.'
}
$padded = $valid.Clone()
for ($i = 81; $i -lt 104; $i++) { $padded[$i] = 32 }
for ($i = 130; $i -lt 152; $i++) { $padded[$i] = 32 }
if ([StorageWindowsCsmiRaid]::Decode($padded, $padded.Length, 0, 2, 0, 81) -ne 2) {
    throw 'Fixed-width padded candidate fields failed.'
}
$refusals = 0
foreach ($case in @(
    @{ Offset = 0; Value = 27 },
    @{ Offset = 4; Value = 0 },
    @{ Offset = 16; Value = 10 },
    @{ Offset = 20; Value = 1 },
    @{ Offset = 24; Value = 0 },
    @{ Offset = 28; Value = 1 },
    @{ Offset = 43; Value = 0 },
    @{ Offset = 43; Value = 7 },
    @{ Offset = 44; Value = 1 },
    @{ Offset = 162; Value = 1 }
)) {
    $bytes = $valid.Clone()
    [BitConverter]::GetBytes([uint32]$case.Value).CopyTo($bytes, $case.Offset)
    try { [StorageWindowsCsmiRaid]::Decode($bytes, $bytes.Length, 0, 2, 0, 81) | Out-Null }
    catch [System.Management.Automation.MethodInvocationException] {
        if ($_.Exception.InnerException -isnot [IO.IOException] -and
            $_.Exception.InnerException -isnot [NotSupportedException]) { throw }
        $refusals++
    }
}
foreach ($kind in 'duplicate', 'missing', 'encoding', 'truncated', 'new-width', 'version', 'unused', 'partial-tail', 'shifted-member') {
    $bytes = $valid.Clone()
    $used = $bytes.Length
    $minor = 81
    $capacity = 2
    switch ($kind) {
        'duplicate' { [Array]::Copy($bytes, 112, $bytes, 240, 40) }
        'missing' { [Array]::Clear($bytes, 112, 40) }
        'encoding' { $bytes[112] = 255 }
        'truncated' { $used-- }
        'new-width' {
            $bytes = [byte[]]::new(352)
            [Array]::Copy($valid, $bytes, $valid.Length)
            [BitConverter]::GetBytes([uint32]324).CopyTo($bytes, 24)
            $used = $bytes.Length
        }
        'version' { $minor = 90 }
        'unused' {
            $bytes = $spare.Clone()
            $bytes[320] = 1
            $used = $bytes.Length
            $capacity = 6
        }
        'partial-tail' {
            $bytes = New-Response 1 2
            $used = 319
            [BitConverter]::GetBytes([uint32]291).CopyTo($bytes, 24)
        }
        'shifted-member' {
            $bytes = New-Response 2 3
            [Array]::Clear($bytes, 192, 128)
            [Array]::Copy($valid, 192, $bytes, 200, 128)
            $used = $bytes.Length
            $capacity = 3
        }
    }
    try { [StorageWindowsCsmiRaid]::Decode($bytes, $used, 0, $capacity, 0, $minor) | Out-Null }
    catch [System.Management.Automation.MethodInvocationException] {
        if ($_.Exception.InnerException -isnot [IO.IOException] -and
            $_.Exception.InnerException -isnot [NotSupportedException]) { throw }
        $refusals++
    }
}
if ($refusals -ne 19) { throw 'Required historical candidate refusals did not run.' }
$audit = $null
if ($InputEvidencePath) {
    $stream = [IO.File]::Open($InputEvidencePath, [IO.FileMode]::Open,
        [IO.FileAccess]::Read, [IO.FileShare]::Read)
    try {
        if ($stream.Length -gt 1048576) { throw 'Private evidence exceeds the offline audit bound.' }
        $reader = [IO.StreamReader]::new($stream)
        try { $evidence = $reader.ReadToEnd() | ConvertFrom-Json }
        finally { $reader.Dispose() }
    }
    finally { $stream.Dispose() }
    $commands = @($evidence.csmi.Commands)
    if (($commands.Command -join ',') -ne '1,10,11' -or
        $evidence.binding_admitted -ne $false -or
        -not $evidence.observation_rechecked) {
        throw 'Expected the isolated three-command private response evidence.'
    }
    $captured = [Convert]::FromBase64String($commands[2].BytesBase64)
    if ($captured.Length -ne 448 -or $commands[2].ReturnedBytes -ne 448 -or
        $commands[2].Win32Error -ne 0 -or $commands[2].ReturnCode -ne 0) {
        throw 'Expected the recorded successful 448-byte native response.'
    }
    $failures = @()
    foreach ($alteredCopy in $false, $true) {
        $bytes = $captured.Clone()
        if ($alteredCopy) { [Array]::Clear($bytes, 44, 20) }
        $failure = $null
        try { [StorageWindowsCsmiRaid]::Decode($bytes, $bytes.Length, 0, 3, 0, 81) | Out-Null }
        catch [System.Management.Automation.MethodInvocationException] {
            if ($_.Exception.InnerException -isnot [IO.IOException] -and
                $_.Exception.InnerException -isnot [NotSupportedException]) { throw }
            $failure = $_.Exception.InnerException.Message
        }
        if (-not $failure) { throw 'The unsupported response unexpectedly passed the historical candidate.' }
        $failures += $failure
    }
    $audit = [ordered]@{
        run = $evidence.run
        captured_response_refused = $true
        refusal = $failures[0]
        adversarial_prefix_cleared_copy_refused = $true
        adversarial_copy_refusal = $failures[1]
        evidence_opened_read_only = $true
        binding_admitted = $false
    }
}
[ordered]@{
    result = 'historical_raid_candidate_codec_passed'
    refusals = $refusals
    maximum_members = 6
    fixed_width_padding = $true
    native_collection = $false
    command_sender_present = $false
    binding_admitted = $false
    captured_response_audit = $audit
    codec_sha256 = (Get-FileHash -LiteralPath (Join-Path $PSScriptRoot 'storage-windows-csmi-raid.cs')).Hash
    harness_sha256 = (Get-FileHash -LiteralPath $PSCommandPath).Hash
} | ConvertTo-Json -Depth 4

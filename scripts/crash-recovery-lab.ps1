[CmdletBinding()]
param(
    [ValidateSet('initialize', 'run', 'continue', 'self-test')]
    [string] $Action = 'run',
    [ValidateSet('publication_full', 'receive_full', 'normalization_full', 'database_full', 'backup_detached', 'backup_wrong_volume')]
    [string] $Case,
    [string] $Consent,
    [string] $Descriptor,
    [string] $Root,
    [string] $Image,
    [string] $Evidence,
    [string] $Run,
    [string] $Replacement,
    [string] $ReplacementImage,
    [ValidateSet('detach-backup', 'swap-backup', 'return-original-backup')]
    [string] $Step
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Write-ContinuationSignal([string] $Directory, [string] $Step, [string] $Run) {
    $signal = Join-Path $Directory "$Step.continue"
    $temporary = Join-Path $Directory ".$Step.$([Guid]::NewGuid().ToString('N')).pending"
    $stream = [IO.File]::Open($temporary, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
    try {
        try {
            $bytes = [Text.Encoding]::UTF8.GetBytes($Run)
            $stream.Write($bytes, 0, $bytes.Length)
            $stream.Flush($true)
        } finally { $stream.Dispose() }
        [IO.File]::Move($temporary, $signal)
    } finally { [IO.File]::Delete($temporary) }
}

if ($Action -eq 'self-test') {
    $tokens = $null
    $errors = $null
    [System.Management.Automation.Language.Parser]::ParseFile($PSCommandPath, [ref]$tokens, [ref]$errors) | Out-Null
    if ($errors.Count) { throw ($errors | Out-String) }
    foreach ($arguments in @(
        @('-Action', 'run', '-Case', 'publication_full'),
        @('-Action', 'run', '-Case', 'publication_full', '-Consent', 'wrong'),
        @('-Action', 'run', '-Case', 'crash_child'),
        @('-Action', 'initialize', '-Consent', 'disposable-volume-faults'),
        @('-Action', 'continue', '-Consent', 'disposable-volume-faults')
    )) {
        $start = [Diagnostics.ProcessStartInfo]::new()
        $start.FileName = 'powershell.exe'
        $start.Arguments = "-NoProfile -NonInteractive -File `"$PSCommandPath`" $($arguments -join ' ')"
        $start.UseShellExecute = $false
        $start.RedirectStandardOutput = $true
        $start.RedirectStandardError = $true
        $process = [Diagnostics.Process]::Start($start)
        try {
            $null = $process.StandardOutput.ReadToEnd()
            $null = $process.StandardError.ReadToEnd()
            $process.WaitForExit()
            if ($process.ExitCode -eq 0) { throw 'Unsafe wrapper arguments were accepted.' }
        } finally { $process.Dispose() }
    }
    $directory = Join-Path ([IO.Path]::GetTempPath()) "domus-signal-$([Guid]::NewGuid().ToString('N'))"
    [IO.Directory]::CreateDirectory($directory) | Out-Null
    $signal = Join-Path $directory 'detach-backup.continue'
    try {
        Write-ContinuationSignal $directory 'detach-backup' 'generated-case'
        if ([IO.File]::ReadAllText($signal) -cne 'generated-case') { throw 'Signal content was not published.' }
        $rejected = $false
        try { Write-ContinuationSignal $directory 'detach-backup' 'different-run' }
        catch [IO.IOException] { $rejected = $true }
        if (-not $rejected -or [IO.File]::ReadAllText($signal) -cne 'generated-case') {
            throw 'An existing signal was replaced.'
        }
        if (@(Get-ChildItem -LiteralPath $directory -Force).Count -ne 1) { throw 'A temporary signal was retained.' }
    } finally {
        [IO.File]::Delete($signal)
        [IO.Directory]::Delete($directory)
    }
    Write-Output 'Lab wrapper rejection and signal checks passed. No host configuration changes were made.'
    exit 0
}

if ($Consent -ne 'disposable-volume-faults') {
    throw 'Explicit -Consent disposable-volume-faults is required.'
}
if ($Action -eq 'continue') {
    if (-not $Descriptor -or -not $Step) { throw 'Descriptor and Step are required.' }
    $lab = Get-Content -LiteralPath $Descriptor -Raw | ConvertFrom-Json
    $parent = [IO.Path]::GetDirectoryName([IO.Path]::GetFullPath($Descriptor))
    if ($lab.consent -ne $Consent -or $lab.run -notmatch '^[A-Za-z0-9-]{1,64}$' -or
        -not [String]::Equals($parent, [IO.Path]::GetFullPath($lab.evidence.path), [StringComparison]::OrdinalIgnoreCase)) {
        throw 'Descriptor must be in its bound evidence directory.'
    }
    $entry = Get-Item -LiteralPath $parent
    while ($null -ne $entry) {
        if (($entry.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw 'Reparse paths are not permitted.' }
        $entry = $entry.Parent
    }
    $request = Join-Path $lab.evidence.path "$Step.request.json"
    if (((Get-Item -LiteralPath $request).Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw 'Request must not be a reparse point.'
    }
    $pending = Get-Content -LiteralPath $request -Raw | ConvertFrom-Json
    if ($pending.run -ne $lab.run -or $pending.step -ne $Step) { throw 'The operator request does not match this run.' }
    Write-ContinuationSignal $lab.evidence.path $Step $lab.run
    exit 0
}

$names = @(
    'DOMUS_LAB_CONSENT', 'DOMUS_LAB_DESCRIPTOR', 'DOMUS_LAB_ROOT', 'DOMUS_LAB_IMAGE',
    'DOMUS_LAB_EVIDENCE', 'DOMUS_LAB_RUN', 'DOMUS_LAB_REPLACEMENT', 'DOMUS_LAB_REPLACEMENT_IMAGE'
)
$previous = @{}
foreach ($name in $names) {
    $previous[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
    [Environment]::SetEnvironmentVariable($name, $null, 'Process')
}
try {
    $env:DOMUS_LAB_CONSENT = $Consent
    if ($Action -eq 'initialize') {
        if (-not $Root -or -not $Image -or -not $Evidence -or -not $Run) {
            throw 'Root, Image, Evidence, and Run are required.'
        }
        $env:DOMUS_LAB_ROOT = $Root
        $env:DOMUS_LAB_IMAGE = $Image
        $env:DOMUS_LAB_EVIDENCE = $Evidence
        $env:DOMUS_LAB_RUN = $Run
        $env:DOMUS_LAB_REPLACEMENT = $Replacement
        $env:DOMUS_LAB_REPLACEMENT_IMAGE = $ReplacementImage
        $test = 'volume_lab_initialize'
    } else {
        if (-not $Descriptor -or -not $Case) { throw 'Descriptor and Case are required.' }
        $env:DOMUS_LAB_DESCRIPTOR = $Descriptor
        $test = "volume_lab_$Case"
    }
    $launcher = Join-Path (Split-Path $PSScriptRoot -Parent) 'dev.ps1'
    & $launcher test --lib "crash_tests::album::lab::$test" -- --exact --ignored --nocapture
    if ($LASTEXITCODE -ne 0) { throw "Lab $Action failed. Retain all fixture evidence." }
} finally {
    foreach ($name in $names) {
        [Environment]::SetEnvironmentVariable($name, $previous[$name], 'Process')
    }
}

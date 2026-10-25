[CmdletBinding()]
param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]] $TaskArgs = @('check')
)

$ErrorActionPreference = 'Stop'
Set-Location -LiteralPath $PSScriptRoot
foreach ($argument in $TaskArgs) {
    if ($argument -notmatch '^[A-Za-z0-9_.:-]+$') {
        throw "Unsupported task argument: $argument"
    }
}
if ((& rustc +stable --version) -notmatch '^rustc 1\.99\.1 ') {
    throw 'This launcher requires installed stable Rust 1.99.1. It does not install tools.'
}
if ($env:PNPM_HOME -and (Test-Path -LiteralPath (Join-Path $env:PNPM_HOME 'bin'))) {
    $env:Path = (Join-Path $env:PNPM_HOME 'bin') + ';' + $env:Path
}

if ($IsWindows -or $PSVersionTable.PSEdition -eq 'Desktop') {
    $installer = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer'
    $vswhere = Join-Path $installer 'vswhere.exe'
    if (-not (Test-Path -LiteralPath $vswhere)) {
        throw 'Visual Studio Installer discovery is unavailable. Use a configured Developer PowerShell.'
    }
    $installations = & $vswhere -all -products '*' -format json | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'Visual Studio discovery failed.' }
    $vcvars = $null
    foreach ($installation in $installations) {
        $build = Join-Path $installation.installationPath 'VC\Auxiliary\Build'
        if ((Test-Path -LiteralPath (Join-Path $build 'vcvars64.bat')) -and
            (Test-Path -LiteralPath (Join-Path $build 'vcvarsall.bat'))) {
            $vcvars = Join-Path $build 'vcvars64.bat'
            break
        }
    }
    if (-not $vcvars) { throw 'A complete x64 MSVC build environment was not found.' }
    $env:Path = $installer + ';' + $env:Path
    & $env:ComSpec /d /c "call `"$vcvars`" >nul && cargo +stable task $($TaskArgs -join ' ')"
} else {
    & cargo +stable task @TaskArgs
}
exit $LASTEXITCODE

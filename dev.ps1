[CmdletBinding()]
param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]] $TaskArgs = @('check')
)

$ErrorActionPreference = 'Stop'

function Invoke-PinnedTask {
    param([string] $Toolchain, [string[]] $Arguments)
    & rustup run $Toolchain cargo task @Arguments
}

function Get-PinnedRustToolchain {
    param([string] $Pin)
    if ($Pin -notmatch '^\d+\.\d+\.\d+$') {
        throw 'The Rust toolchain pin must be an exact release version.'
    }
    $escaped = [regex]::Escape($Pin)
    $installed = @(& rustup toolchain list)
    if ($LASTEXITCODE -ne 0) { throw 'Rust toolchain discovery failed.' }
    $toolchain = 'stable'
    if ($installed -match "^$escaped(?:-|\s|$)") { $toolchain = $Pin }
    $version = & rustup run $toolchain rustc --version
    if ($LASTEXITCODE -ne 0 -or $version -notmatch "^rustc $escaped ") {
        throw "This launcher requires installed Rust $Pin. The stable alias is accepted only at that exact version. It does not install tools."
    }
    return $toolchain
}

Set-Location -LiteralPath $PSScriptRoot
if ($TaskArgs.Count -eq 0) { throw 'Select a developer task.' }
$pin = [regex]::Match(
    (Get-Content -LiteralPath (Join-Path $PSScriptRoot 'rust-toolchain.toml') -Raw),
    '(?m)^channel\s*=\s*"(\d+\.\d+\.\d+)"\s*$'
).Groups[1].Value
$toolchain = Get-PinnedRustToolchain -Pin $pin
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
    $environment = & $env:ComSpec /d /c "call `"$vcvars`" >nul && set"
    if ($LASTEXITCODE -ne 0) { throw 'MSVC environment setup failed.' }
    foreach ($entry in $environment) {
        if ($entry -match '^([^=]+)=(.*)$') {
            [Environment]::SetEnvironmentVariable($Matches[1], $Matches[2], 'Process')
        }
    }
}
Invoke-PinnedTask -Toolchain $toolchain -Arguments $TaskArgs
exit $LASTEXITCODE

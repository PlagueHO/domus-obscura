$ErrorActionPreference = 'Stop'
$tokens = $null
$errors = $null
$launcher = Join-Path (Split-Path $PSScriptRoot -Parent) 'dev.ps1'
$ast = [System.Management.Automation.Language.Parser]::ParseFile(
    $launcher, [ref]$tokens, [ref]$errors
)
if ($errors.Count) { throw ($errors | Out-String) }
$selection = $ast.Find({
    param($node)
    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
        $node.Name -eq 'Get-PinnedRustToolchain'
}, $false)
if (-not $selection) { throw 'Pinned toolchain selection function is missing.' }
. ([scriptblock]::Create($selection.Extent.Text))

function rustup {
    param([Parameter(ValueFromRemainingArguments = $true)][string[]] $CommandArgs)
    $script:calls.Add($CommandArgs -join ' ')
    $global:LASTEXITCODE = 0
    if (($CommandArgs -join ' ') -eq 'toolchain list') {
        return $script:installed
    }
    if ($CommandArgs.Count -eq 4 -and $CommandArgs[0] -eq 'run' -and
        $CommandArgs[2] -eq 'rustc' -and $CommandArgs[3] -eq '--version') {
        if ($script:versions.ContainsKey($CommandArgs[1])) {
            return $script:versions[$CommandArgs[1]]
        }
        $global:LASTEXITCODE = 1
        return ''
    }
    throw "Unexpected toolchain command: $($CommandArgs -join ' ')"
}

foreach ($case in @(
    @{
        Installed = @('stable-x86_64-pc-windows-msvc', '1.99.1-x86_64-pc-windows-msvc')
        Versions = @{ '1.99.1' = 'rustc 1.99.1 (fixture)'; stable = 'rustc 1.99.2 (fixture)' }
        Expected = '1.99.1'
    },
    @{
        Installed = @('stable-x86_64-pc-windows-msvc')
        Versions = @{ stable = 'rustc 1.99.1 (fixture)' }
        Expected = 'stable'
    },
    @{
        Installed = @('stable-x86_64-pc-windows-msvc')
        Versions = @{ stable = 'rustc 1.99.2 (fixture)' }
        Expected = $null
    },
    @{
        Installed = @()
        Versions = @{}
        Expected = $null
    }
)) {
    $script:installed = $case.Installed
    $script:versions = $case.Versions
    $script:calls = [System.Collections.Generic.List[string]]::new()
    $actual = $null
    $failure = $null
    try { $actual = Get-PinnedRustToolchain -Pin '1.99.1' }
    catch { $failure = $_ }
    if ($case.Expected) {
        if ($failure -or $actual -ne $case.Expected) {
            throw "Wrong installed toolchain selection: $actual; $failure"
        }
    } elseif (-not $failure -or $actual) {
        throw 'Missing or mismatched Rust was accepted.'
    }
    if ($script:calls.Count -ne 2 -or
        $script:calls[0] -ne 'toolchain list' -or
        $script:calls[1] -notmatch '^run (1\.99\.1|stable) rustc --version$') {
        throw "Unexpected commands or attempted installation: $($script:calls -join '; ')"
    }
}
$global:LASTEXITCODE = 0
Write-Output 'Pinned launcher toolchain tests passed without host changes.'

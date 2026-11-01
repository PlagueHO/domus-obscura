[CmdletBinding()]
param([string] $NsisCompiler)

$ErrorActionPreference = 'Stop'
$root = Join-Path ([IO.Path]::GetTempPath()) ('domus-service-helper-' + [Guid]::NewGuid().ToString('N'))
$script:services = @{}
$script:events = [Collections.Generic.List[string]]::new()
$script:failPolicy = $false
$script:users = @{}
$script:administrators = @()
$originalProgramFiles = $env:ProgramFiles
New-Item -ItemType Directory -Path $root | Out-Null

function Get-CimInstance {
    param([string] $ClassName, [string] $Filter)
    if ($ClassName -eq 'Win32_Process') { return @() }
    if ($ClassName -ne 'Win32_Service' -or $Filter -notmatch "^Name='([^']+)'$") {
        throw 'Unexpected service helper query.'
    }
    return $script:services[$Matches[1]]
}

function Get-LocalUser {
    param([string] $Name)
    if (-not $script:users.ContainsKey($Name)) { throw 'Generated account is missing.' }
    return $script:users[$Name]
}

function Get-LocalGroupMember {
    param([string] $SID)
    if ($SID -ne 'S-1-5-32-544') { throw 'Unexpected group query.' }
    return $script:administrators
}

try {
    . (Join-Path $PSScriptRoot 'services.ps1') -Action Status -InstallationDirectory $root | Out-Null

    function Invoke-ServiceControl {
        param([string[]] $Arguments)
        $script:events.Add('sc:' + ($Arguments -join ':'))
        if ($Arguments[0] -eq 'start') { $script:services[$Arguments[1]].State = 'Running' }
        if ($Arguments[0] -eq 'stop') { $script:services[$Arguments[1]].State = 'Stopped' }
    }

    function Invoke-ServicePolicy {
        param([string] $Name, [string] $Policy)
        $script:events.Add("policy:${Name}:${Policy}")
        if ($script:failPolicy) { throw 'Generated policy failure.' }
    }

    $tokens = $null
    $errors = $null
    $ast = [Management.Automation.Language.Parser]::ParseFile(
        (Join-Path $PSScriptRoot 'services.ps1'), [ref] $tokens, [ref] $errors)
    if ($errors) { throw 'Service helper syntax is invalid.' }
    $switch = $ast.Find({
        param($node)
        $node -is [Management.Automation.Language.SwitchStatementAst]
    }, $true)

    function Invoke-TestAction {
        param([string] $Name)
        $clause = @($switch.Clauses | Where-Object { $_.Item1.SafeGetValue() -eq $Name })
        if ($clause.Count -ne 1) { throw 'Expected one service action.' }
        $text = $clause[0].Item2.Extent.Text
        & ([scriptblock]::Create($text.Substring(1, $text.Length - 2)))
    }

    function Assert-Events {
        param([string[]] $Expected)
        if (($script:events -join '|') -ne ($Expected -join '|')) {
            throw ('Unexpected helper sequence: ' + ($script:events -join '|'))
        }
        $script:events.Clear()
    }

    function Assert-BlockedStart {
        param([string] $Message)
        $failed = $false
        try { Invoke-TestAction 'Start' } catch { $failed = $true }
        if (-not $failed) { throw $Message }
        Assert-Events @()
    }

    foreach ($name in @('fixture', 'other-fixture')) {
        $rid = if ($name -eq 'fixture') { '1001' } else { '1002' }
        $script:users[$name] = [PSCustomObject]@{
            Name = $name
            Enabled = $true
            SID = [PSCustomObject]@{ Value = "S-1-5-21-1-2-3-$rid" }
        }
    }
    foreach ($role in @(@($Api, 'api'), @($Worker, 'worker'))) {
        $script:services[$role[0]] = [PSCustomObject]@{
            PathName = '"' + $Executable + '" windows-service ' + $role[1] + ' "fixture.json"'
            StartName = '.\fixture'
            State = 'Stopped'
            ExitCode = 0
            ServiceSpecificExitCode = 0
        }
    }

    Invoke-TestAction 'Start'
    Assert-Events @(
        'sc:start:DomusObscuraApi', 'sc:start:DomusObscuraWorker',
        'policy:DomusObscuraApi:enable', 'sc:config:DomusObscuraApi:start=:auto',
        'policy:DomusObscuraWorker:enable', 'sc:config:DomusObscuraWorker:start=:auto')

    Invoke-TestAction 'Stop'
    Assert-Events @(
        'sc:config:DomusObscuraWorker:start=:demand', 'policy:DomusObscuraWorker:disable',
        'sc:config:DomusObscuraApi:start=:demand', 'policy:DomusObscuraApi:disable',
        'sc:stop:DomusObscuraWorker', 'sc:stop:DomusObscuraApi')

    $script:failPolicy = $true
    $failed = $false
    try { Invoke-TestAction 'Stop' } catch { $failed = $true }
    if (-not $failed) { throw 'Policy failure was hidden.' }
    Assert-Events @('sc:config:DomusObscuraWorker:start=:demand', 'policy:DomusObscuraWorker:disable')
    $script:failPolicy = $false

    foreach ($account in @('LocalSystem', 'NT AUTHORITY\LocalService', '.\other-fixture')) {
        $script:services[$Worker].StartName = $account
        Assert-BlockedStart 'An invalid or different service account was accepted.'
    }
    $script:services[$Worker].StartName = '.\fixture'
    $script:users['fixture'].Enabled = $false
    Assert-BlockedStart 'A disabled runtime account was accepted.'
    $script:users['fixture'].Enabled = $true
    $script:administrators = @($script:users['fixture'])
    Assert-BlockedStart 'An administrator runtime account was accepted.'
    $script:administrators = @()
    $script:services[$Worker].PathName = '"' + $Executable + '" windows-service api "fixture.json"'
    Assert-BlockedStart 'The API started before worker ownership was checked.'
    $script:services[$Worker].PathName = '"' + $Executable + '" windows-service worker "fixture.json"'

    $script:services[$Api].PathName = '"C:\another-installation\domus.exe" windows-service api "fixture.json"'
    Assert-BlockedStart 'Another installation was accepted.'
    $script:services[$Api].PathName = '"' + $Executable + '" windows-service worker "fixture.json"'
    Assert-BlockedStart 'A service with the wrong application role was accepted.'

    $script:services.Clear()
    $browser = Join-Path $root 'browser'
    New-Item -ItemType Directory -Path $browser | Out-Null
    [IO.File]::WriteAllText($Executable, 'generated executable fixture')
    [IO.File]::WriteAllText((Join-Path $browser 'index.html'), 'generated browser fixture')
    [IO.File]::WriteAllText((Join-Path $browser 'unrelated.txt'), 'retain')
    $owned = @{}
    foreach ($name in @('domus.exe', 'browser/index.html')) {
        $owned[$name] = (Get-FileHash -LiteralPath (Join-Path $root $name) -Algorithm SHA256).Hash.ToLowerInvariant()
    }
    $manifest = @{ format = 1; target = 'x86_64-pc-windows-msvc'; files = $owned }
    [IO.File]::WriteAllText((Join-Path $root 'package-manifest.json'), ($manifest | ConvertTo-Json -Depth 5))
    [IO.File]::WriteAllText((Join-Path $root 'Uninstall.exe'), 'generated uninstaller fixture')
    Invoke-TestAction 'RecordInstall'
    Invoke-TestAction 'ValidateUninstall'
    Assert-Events @()
    $IncomingManifest = Join-Path $root 'incoming-manifest.json'
    [IO.File]::WriteAllText($IncomingManifest, ($manifest | ConvertTo-Json -Depth 5))

    function Assert-BlockedPreparation {
        param([string] $Reason, [string] $Action = 'PrepareUpdate')
        $before = @{}
        foreach ($name in @('domus.exe', 'browser/index.html', 'browser/unrelated.txt', 'Uninstall.exe', 'package-manifest.json')) {
            $before[$name] = (Get-FileHash -LiteralPath (Join-Path $root $name) -Algorithm SHA256).Hash
        }
        $failure = $null
        try { Invoke-TestAction $Action } catch { $failure = $_.Exception.Message }
        if (-not $failure -or $failure -notmatch $Reason) {
            throw "$Action did not report the expected failure: $Reason. Actual: $failure"
        }
        foreach ($name in $before.Keys) {
            if ((Get-FileHash -LiteralPath (Join-Path $root $name) -Algorithm SHA256).Hash -ne $before[$name]) {
                throw 'A rejected preparation changed an existing file.'
            }
        }
    }

    [IO.File]::WriteAllText($Executable, 'changed fixture')
    Assert-BlockedPreparation 'installer-owned file changed'
    Assert-BlockedPreparation 'installer-owned file changed' 'ValidateUninstall'
    [IO.File]::WriteAllText($Executable, 'generated executable fixture')
    [IO.File]::WriteAllText((Join-Path $root 'Uninstall.exe'), 'changed uninstaller fixture')
    Assert-BlockedPreparation 'installer-owned file changed'
    Assert-BlockedPreparation 'installer-owned file changed' 'ValidateUninstall'
    [IO.File]::WriteAllText((Join-Path $root 'Uninstall.exe'), 'generated uninstaller fixture')

    $record = [IO.File]::ReadAllText((Join-Path $root 'package-manifest.json'))
    [IO.File]::WriteAllText((Join-Path $root 'package-manifest.json'), ($manifest | ConvertTo-Json -Depth 5))
    Assert-BlockedPreparation 'unrelated file or directory'
    Assert-BlockedPreparation 'uninstaller ownership record is missing' 'ValidateUninstall'
    [IO.File]::WriteAllText((Join-Path $root 'package-manifest.json'), $record)
    $incomingPath = $IncomingManifest
    $IncomingManifest = ''
    Assert-BlockedPreparation 'requires the incoming package manifest'
    $IncomingManifest = $incomingPath

    foreach ($name in @('browser/unrelated.txt', 'browser', 'browser/unrelated.txt/new-file.txt', '../escape.txt', 'browser/./alias.txt', 'CON.txt')) {
        $incomingFiles = $owned.Clone()
        $incomingFiles[$name] = $owned['domus.exe']
        $incoming = @{ format = 1; target = 'x86_64-pc-windows-msvc'; files = $incomingFiles }
        [IO.File]::WriteAllText($IncomingManifest, ($incoming | ConvertTo-Json -Depth 5))
        $reason = if ($name -in @('browser/unrelated.txt', 'browser')) { 'unrelated file or directory' }
            elseif ($name -eq 'browser/unrelated.txt/new-file.txt') { 'blocked by a file' }
            else { 'unsafe relative path' }
        Assert-BlockedPreparation $reason
    }
    $linkedTarget = Join-Path $root 'incoming-target'
    New-Item -ItemType Directory -Path $linkedTarget | Out-Null
    [IO.File]::WriteAllText((Join-Path $linkedTarget 'new-file.txt'), 'untouched target')
    $incomingLink = Join-Path $root 'incoming-link'
    New-Item -ItemType Junction -Path $incomingLink -Target $linkedTarget | Out-Null
    $incomingFiles = $owned.Clone()
    $incomingFiles['incoming-link/new-file.txt'] = $owned['domus.exe']
    $incoming = @{ format = 1; target = 'x86_64-pc-windows-msvc'; files = $incomingFiles }
    [IO.File]::WriteAllText($IncomingManifest, ($incoming | ConvertTo-Json -Depth 5))
    Assert-BlockedPreparation 'reparse point'
    if ([IO.File]::ReadAllText((Join-Path $linkedTarget 'new-file.txt')) -ne 'untouched target') {
        throw 'Update changed a reparse target.'
    }
    [IO.Directory]::Delete($incomingLink)

    $savedBrowser = Join-Path $root 'saved-browser'
    $uninstallTarget = Join-Path $root 'uninstall-target'
    Move-Item -LiteralPath $browser -Destination $savedBrowser
    Copy-Item -LiteralPath $savedBrowser -Destination $uninstallTarget -Recurse
    New-Item -ItemType Junction -Path $browser -Target $uninstallTarget | Out-Null
    try {
        Assert-BlockedPreparation 'reparse point' 'ValidateUninstall'
        if ([IO.File]::ReadAllText((Join-Path $uninstallTarget 'unrelated.txt')) -ne 'retain') {
            throw 'Uninstall validation changed a reparse target.'
        }
    } finally {
        [IO.Directory]::Delete($browser)
        Move-Item -LiteralPath $savedBrowser -Destination $browser
    }

    [IO.File]::WriteAllText($IncomingManifest, ($manifest | ConvertTo-Json -Depth 5))
    Invoke-TestAction 'PrepareUpdate'
    if ((Test-Path -LiteralPath $Executable) -or
        (Test-Path -LiteralPath (Join-Path $root 'Uninstall.exe')) -or
        (Test-Path -LiteralPath (Join-Path $browser 'index.html')) -or
        [IO.File]::ReadAllText((Join-Path $browser 'unrelated.txt')) -ne 'retain') {
        throw 'Update did not remove exact owned files and retain unrelated files.'
    }
    Assert-Events @()

    $emptyParent = Join-Path $root 'empty-install'
    New-Item -ItemType Directory -Path $emptyParent | Out-Null
    $env:ProgramFiles = $emptyParent
    $emptyInstall = Join-Path $emptyParent 'Domus Obscura'
    . (Join-Path $PSScriptRoot 'services.ps1') -Action CheckInstallPath -InstallationDirectory $emptyInstall
    if (Test-Path -LiteralPath $emptyInstall) { throw 'First-install validation created a directory.' }

    $linkParent = Join-Path $root 'linked-install'
    $target = Join-Path $root 'generated-target'
    New-Item -ItemType Directory -Path $linkParent,$target | Out-Null
    $link = Join-Path $linkParent 'Domus Obscura'
    New-Item -ItemType Junction -Path $link -Target $target | Out-Null
    $env:ProgramFiles = $linkParent
    $failure = $null
    try {
        . (Join-Path $PSScriptRoot 'services.ps1') -Action CheckInstallPath -InstallationDirectory $link
    } catch { $failure = $_.Exception.Message }
    if (-not $failure -or $failure -notmatch 'reparse point') {
        throw "An empty first-install junction was not rejected: $failure"
    }
    [IO.Directory]::Delete($link)

    $installer = [IO.File]::ReadAllText((Join-Path $PSScriptRoot 'installer.nsi'))
    if ($installer -match '(?m)^\s*(ExecWait|Delete)\b') {
        throw 'Installer commands must use the checked command macros.'
    }
    if ($NsisCompiler) {
        $version = & $NsisCompiler /VERSION
        if ($LASTEXITCODE -ne 0 -or $version -ne 'v3.13') {
            throw 'Command guard fixtures require NSIS 3.13.'
        }
        $source = Join-Path $root 'checked-command-fixture.nsi'
        [IO.File]::WriteAllText($source, @'
Unicode true
!include "${GUARDS}"
Name "Generated command guard fixture"
OutFile "${OUTPUT}"
RequestExecutionLevel user
SilentInstall silent
Section
    StrCpy $0 0
!ifdef CASE_LAUNCH_FAIL
    !insertmacro ExecChecked '"$EXEDIR\absent-command.exe"' unexpected expected
!else ifdef CASE_EXIT_FAIL
    !insertmacro ExecChecked '"$SYSDIR\cmd.exe" /c exit 7' unexpected expected
!else ifdef CASE_EXIT_OK
    !insertmacro ExecChecked '"$SYSDIR\cmd.exe" /c exit 0' expected unexpected
!else ifdef CASE_DELETE_FAIL
    !insertmacro DeleteChecked "$EXEDIR\locked.fixture" expected
    Goto unexpected
!else
    !error "A command guard fixture case is required"
!endif
unexpected:
    SetErrorLevel 99
    Quit
expected:
    SetErrorLevel 0
SectionEnd
'@)
        $locked = [IO.File]::Open(
            (Join-Path $root 'locked.fixture'), [IO.FileMode]::Create,
            [IO.FileAccess]::ReadWrite, [IO.FileShare]::Read)
        try {
            foreach ($case in @('LAUNCH_FAIL', 'EXIT_FAIL', 'EXIT_OK', 'DELETE_FAIL')) {
                $output = Join-Path $root ("checked-command-$case.exe")
                & $NsisCompiler /NOCONFIG /WX /V2 "/DGUARDS=$(Join-Path $PSScriptRoot 'checked-commands.nsh')" "/DOUTPUT=$output" "/DCASE_$case" $source
                if ($LASTEXITCODE -ne 0) { throw "Command guard fixture compilation failed: $case" }
                $start = [Diagnostics.ProcessStartInfo]::new()
                $start.FileName = $output
                $start.WorkingDirectory = $root
                $start.UseShellExecute = $false
                $probe = [Diagnostics.Process]::Start($start)
                try {
                    if (-not $probe.WaitForExit(30000)) {
                        Stop-Process -Id $probe.Id
                        $probe.WaitForExit()
                        throw "Command guard fixture timed out: $case"
                    }
                    if ($probe.ExitCode -ne 0) { throw "Command guard accepted an incorrect result: $case" }
                } finally {
                    $probe.Dispose()
                }
            }
            if (-not (Test-Path -LiteralPath (Join-Path $root 'locked.fixture'))) {
                throw 'A rejected deletion removed the locked fixture.'
            }
        } finally {
            $locked.Dispose()
        }
        Write-Output 'Generated NSIS command guards passed launch failure, nonzero exit, successful exit and locked-file deletion checks. No application installation was performed.'
    } else {
        Write-Output 'NSIS command guard runtime checks were not run. Supply -NsisCompiler with the pinned compiler to run them.'
    }
    Write-Output 'Service order, account/role preflight, failure handling, update collisions, uninstall integrity/reparse checks, exact cleanup and first-install path checks passed with generated SCM/accounts and fresh files. No native services or accounts were changed.'
} finally {
    $env:ProgramFiles = $originalProgramFiles
    Remove-Item -LiteralPath $root -Recurse
}

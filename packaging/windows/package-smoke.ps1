[CmdletBinding()]
param([Parameter(Mandatory = $true)][string] $Payload)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSHOME 'Modules\Microsoft.PowerShell.Security\Microsoft.PowerShell.Security.psd1') -ErrorAction Stop
$Payload = (Get-Item -LiteralPath $Payload).FullName
$root = Join-Path ([IO.Path]::GetTempPath()) ('domus-package-check-' + [Guid]::NewGuid().ToString('N'))
$process = $null
$probe = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 8080)
$probe.Start()
$probe.Stop()
New-Item -ItemType Directory -Path $root | Out-Null
try {
    $sid = [Security.Principal.WindowsIdentity]::GetCurrent().User
    $acl = [Security.AccessControl.DirectorySecurity]::new()
    $acl.SetOwner($sid)
    $acl.SetAccessRuleProtection($true, $false)
    foreach ($identity in @($sid, [Security.Principal.SecurityIdentifier]::new('S-1-5-18'), [Security.Principal.SecurityIdentifier]::new('S-1-5-32-544'))) {
        $rule = [Security.AccessControl.FileSystemAccessRule]::new(
            $identity, 'FullControl', 'ContainerInherit,ObjectInherit', 'None', 'Allow')
        $acl.AddAccessRule($rule)
    }
    Set-Acl -LiteralPath $root -AclObject $acl
    $copied = Join-Path $root 'application'
    New-Item -ItemType Directory -Path $copied | Out-Null
    Copy-Item -LiteralPath (Join-Path $Payload 'domus.exe') -Destination $copied
    Copy-Item -LiteralPath (Join-Path $Payload 'browser') -Destination $copied -Recurse
    $index = [IO.File]::ReadAllText((Join-Path $copied 'browser\index.html'))
    $asset = [regex]::Match($index, '/assets/[A-Za-z0-9._-]+\.js').Value
    if (-not $asset) { throw 'Connected browser JavaScript reference is missing.' }

    foreach ($attempt in @(1, 2)) {
        $start = [Diagnostics.ProcessStartInfo]::new()
        $start.FileName = Join-Path $copied 'domus.exe'
        $start.Arguments = 'serve'
        $start.UseShellExecute = $false
        $start.WorkingDirectory = $copied
        $start.EnvironmentVariables['LOCALAPPDATA'] = $root
        $start.EnvironmentVariables['PATH'] = "$env:SystemRoot\System32"
        $process = [Diagnostics.Process]::Start($start)
        $deadline = [DateTime]::UtcNow.AddSeconds(30)
        $ready = $false
        do {
            if ($process.HasExited) { throw "Copied application exited with $($process.ExitCode)." }
            try {
                $session = Invoke-RestMethod -Uri 'http://127.0.0.1:8080/api/session' -TimeoutSec 1
                $ready = ($session | ConvertTo-Json -Depth 10).Contains('_anonymous')
            } catch [System.Net.WebException] {
                if ([DateTime]::UtcNow -ge $deadline) { throw }
            }
            if (-not $ready) { Start-Sleep -Milliseconds 100 }
        } while (-not $ready -and [DateTime]::UtcNow -lt $deadline)
        if (-not $ready) { throw 'Copied application did not expose the anonymous session.' }
        $page = Invoke-WebRequest -Uri 'http://127.0.0.1:8080/' -UseBasicParsing -TimeoutSec 5
        if ($page.StatusCode -ne 200 -or -not $page.Content.Contains($asset)) {
            throw 'Copied application did not serve the packaged connected browser.'
        }
        $script = Invoke-WebRequest -Uri ('http://127.0.0.1:8080' + $asset) -UseBasicParsing -TimeoutSec 5
        if ($script.StatusCode -ne 200 -or $script.RawContentLength -lt 1000) {
            throw 'Copied application did not serve its real JavaScript asset.'
        }
        Stop-Process -Id $process.Id
        $process.WaitForExit()
        $process.Dispose()
        $process = $null
        $database = Join-Path $root 'DomusObscura\state.db'
        $configuration = Join-Path $root 'DomusObscura\service.json'
        if (-not (Test-Path -LiteralPath $database) -or -not (Test-Path -LiteralPath $configuration)) {
            throw 'First-run state was not retained after forced exit.'
        }
        & (Join-Path $copied 'domus.exe') settings $database | Out-Null
        if ($LASTEXITCODE -ne 0) { throw 'Retained SQLite state could not be read.' }
    }
    Write-Output 'Copied package served the real browser twice with fresh state and a system-only PATH. Forced exit retained readable SQLite state. This is not SCM or graceful-stop proof.'
} finally {
    if ($process) {
        if (-not $process.HasExited) { Stop-Process -Id $process.Id }
        $process.WaitForExit()
        $process.Dispose()
    }
    Remove-Item -LiteralPath $root -Recurse
}

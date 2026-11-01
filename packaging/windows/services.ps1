[CmdletBinding()]
param(
    [ValidateSet('Install', 'Start', 'Stop', 'Status', 'Remove', 'CheckStopped', 'CheckInstallPath', 'PrepareUpdate', 'ValidateUninstall', 'RecordInstall')]
    [string] $Action = 'Status',
    [string] $Configuration,
    [string] $IncomingManifest,
    [System.Management.Automation.PSCredential] $Credential,
    [string] $InstallationDirectory = $PSScriptRoot
)

$ErrorActionPreference = 'Stop'
$Api = 'DomusObscuraApi'
$Worker = 'DomusObscuraWorker'
$InstallationDirectory = [IO.Path]::GetFullPath($InstallationDirectory).TrimEnd('\')
$Executable = Join-Path $InstallationDirectory 'domus.exe'

function Find-DirectoryEntry {
    param([string] $Parent, [string] $Name)
    $entries = @(Get-ChildItem -LiteralPath $Parent -Force | Select-Object -First 16385)
    if ($entries.Count -gt 16384) { throw 'The installation directory entry limit was exceeded.' }
    $matching = @($entries | Where-Object { [string]::Equals($_.Name, $Name, [StringComparison]::OrdinalIgnoreCase) })
    if ($matching.Count -gt 1) { throw 'The installation path has ambiguous case variants.' }
    return $matching
}

if ($Action -in @('Install', 'PrepareUpdate', 'ValidateUninstall', 'Remove', 'CheckInstallPath', 'RecordInstall')) {
    $expectedDirectory = Join-Path $env:ProgramFiles 'Domus Obscura'
    if (-not [string]::Equals($InstallationDirectory, $expectedDirectory, [StringComparison]::OrdinalIgnoreCase)) {
        throw 'Install, update preparation, and removal use only the fixed Program Files installation.'
    }
    $parent = [IO.Path]::GetPathRoot($InstallationDirectory)
    foreach ($part in $InstallationDirectory.Substring($parent.Length).Split('\')) {
        $directory = Find-DirectoryEntry $parent $part
        if (-not $directory) { break }
        if (-not $directory.PSIsContainer -or $directory.Attributes -band [IO.FileAttributes]::ReparsePoint) {
            throw 'The installation path must not traverse a reparse point.'
        }
        $parent = $directory.FullName
    }
}

function Invoke-ServiceControl {
    param([string[]] $Arguments)
    & "$env:SystemRoot\System32\sc.exe" @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "Service control failed with exit code $LASTEXITCODE."
    }
}

function Invoke-ServicePolicy {
    param([string] $Name, [string] $Policy)
    $role = if ($Name -eq $Api) { 'api' } else { 'worker' }
    & $Executable windows-service-policy $role $Policy
    if ($LASTEXITCODE -ne 0) {
        throw "Native supervision policy failed for $Name with exit code $LASTEXITCODE."
    }
}

function Get-OwnedService {
    param([string] $Name)
    $service = Get-CimInstance Win32_Service -Filter "Name='$Name'"
    if ($service) {
        $role = if ($Name -eq $Api) { 'api' } else { 'worker' }
        $expected = '"' + $Executable + '" windows-service ' + $role + ' "'
        if (-not $service.PathName.StartsWith($expected, [StringComparison]::OrdinalIgnoreCase)) {
            throw "Service $Name belongs to another installation. It was not changed."
        }
    }
    return $service
}

function Get-DedicatedLocalAccount {
    param([string] $Name)
    if ([IntPtr]::Size -ne 8) { throw 'Service account checks require 64-bit PowerShell.' }
    Import-Module (Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\Modules\Microsoft.PowerShell.LocalAccounts') -ErrorAction Stop
    $parts = $Name.Split('\')
    if ($parts.Count -ne 2 -or $parts[0] -notin @('.', $env:COMPUTERNAME)) {
        throw 'Use a dedicated local account as .\account or COMPUTER\account.'
    }
    $user = Get-LocalUser -Name $parts[1]
    if (-not $user.Enabled -or $user.SID.Value -match '-(500|501)$') {
        throw 'Use an enabled dedicated account, not Administrator or Guest.'
    }
    $administrators = Get-LocalGroupMember -SID 'S-1-5-32-544'
    if ($user.SID.Value -in @($administrators | ForEach-Object { $_.SID.Value })) {
        throw 'The runtime account must not be an administrator.'
    }
    return $user
}

function Assert-ServiceAccounts {
    $accountSid = $null
    foreach ($name in @($Api, $Worker)) {
        $service = Get-OwnedService $name
        if (-not $service) { throw "Service $name is not installed." }
        $user = Get-DedicatedLocalAccount $service.StartName
        if ($accountSid -and $accountSid -ne $user.SID.Value) {
            throw 'Both services must use the same dedicated local account.'
        }
        $accountSid = $user.SID.Value
    }
}

function Assert-Stopped {
    foreach ($name in @($Worker, $Api)) {
        $service = Get-OwnedService $name
        if ($service -and $service.State -ne 'Stopped') {
            throw "Stop $name before updating or removing this installation."
        }
    }
    $running = @(Get-CimInstance Win32_Process | Where-Object {
        $_.ExecutablePath -and [string]::Equals($_.ExecutablePath, $Executable, [StringComparison]::OrdinalIgnoreCase)
    })
    if ($running.Count -ne 0) {
        throw 'A foreground process uses this executable. Stop it before updating or removing files.'
    }
}

function Wait-ServiceState {
    param([string] $Name, [string] $State)
    $deadline = [DateTime]::UtcNow.AddSeconds(180)
    do {
        $service = Get-OwnedService $Name
        if (-not $service) { throw "Service $Name is not installed." }
        if ($service.State -eq $State) { return }
        if ($State -eq 'Running' -and $service.State -eq 'Stopped') {
            throw "Service $Name stopped during startup. Win32 exit: $($service.ExitCode). Service exit: $($service.ServiceSpecificExitCode)."
        }
        Start-Sleep -Milliseconds 250
    } while ([DateTime]::UtcNow -lt $deadline)
    throw "Service $Name did not reach $State. No process was killed. Inspect its status and retained state."
}

function Get-PackageFiles {
    param([string] $Path)
    $item = Get-Item -LiteralPath $Path
    if ($item.PSIsContainer -or $item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw 'The package manifest must be a regular file.'
    }
    $manifest = Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json
    if ($manifest.format -ne 1 -or $manifest.target -ne 'x86_64-pc-windows-msvc' -or
        $manifest.files -isnot [PSCustomObject]) {
        throw 'The package manifest is unsupported. No files were deleted.'
    }
    $entries = @($manifest.files.PSObject.Properties)
    if ($entries.Count -eq 0 -or $entries.Count -gt 8192) {
        throw 'The package file list is invalid.'
    }
    $names = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($entry in $entries) {
        if ($entry.Name -notmatch '^[A-Za-z0-9._/-]+$' -or
            -not $names.Add($entry.Name) -or $entry.Value -isnot [string] -or
            $entry.Value -notmatch '^[a-f0-9]{64}$') {
            throw 'The package has an invalid path or hash. No files were deleted.'
        }
        $parts = $entry.Name.Split('/')
        if ($parts.Count -gt 17) { throw 'The package path depth limit was exceeded.' }
        foreach ($part in $parts) {
            if ($part -in @('', '.', '..') -or $part.EndsWith('.') -or
                $part -match '^(CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])(\.|$)') {
                throw 'The package has an unsafe relative path. No files were deleted.'
            }
        }
    }
    return $entries
}

function Find-InstallEntry {
    param([string] $Name)
    $parts = $Name.Split('/')
    $parent = $InstallationDirectory
    for ($index = 0; $index -lt $parts.Count; $index++) {
        $item = Find-DirectoryEntry $parent $parts[$index]
        if (-not $item) { return $null }
        if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
            throw 'An installed package path is a reparse point. No files were deleted.'
        }
        if ($index -eq $parts.Count - 1) { return $item }
        if (-not $item.PSIsContainer) {
            throw 'An incoming package directory is blocked by a file. No files were deleted.'
        }
        $parent = $item.FullName
    }
}

function Assert-InstalledFile {
    param($Entry)
    $item = Find-InstallEntry $Entry.Name
    if (-not $item -or $item.PSIsContainer -or
        (Get-FileHash -LiteralPath $item.FullName -Algorithm SHA256).Hash.ToLowerInvariant() -ne $Entry.Value) {
        throw 'An installer-owned file changed. Review it before updating. No files were deleted.'
    }
    return $item
}

switch ($Action) {
    'Status' {
        foreach ($name in @($Api, $Worker)) {
            $service = Get-OwnedService $name
            if ($service) {
                $service | Select-Object Name, State, StartMode, StartName, ProcessId, ExitCode, ServiceSpecificExitCode
            } else {
                Write-Output "$name is not installed."
            }
        }
    }
    'CheckStopped' { Assert-Stopped }
    'CheckInstallPath' { Assert-Stopped }
    'ValidateUninstall' {
        Assert-Stopped
        $owned = @(Get-PackageFiles (Join-Path $InstallationDirectory 'package-manifest.json'))
        if ('Uninstall.exe' -notin @($owned.Name)) {
            throw 'The uninstaller ownership record is missing. No files were deleted.'
        }
        foreach ($entry in $owned) { Assert-InstalledFile $entry | Out-Null }
    }
    'PrepareUpdate' {
        Assert-Stopped
        if (-not $IncomingManifest) { throw 'Update preparation requires the incoming package manifest.' }
        $manifestPath = Join-Path $InstallationDirectory 'package-manifest.json'
        $owned = @(Get-PackageFiles $manifestPath)
        $incoming = @(Get-PackageFiles $IncomingManifest)
        $names = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
        $directories = [Collections.Generic.HashSet[string]]::new()
        foreach ($entry in $owned) {
            [void] $names.Add($entry.Name)
            $item = Assert-InstalledFile $entry
            $parent = $item.Directory
            while ($parent -and $parent.FullName -ne $InstallationDirectory) {
                [void] $directories.Add($parent.FullName)
                $parent = $parent.Parent
            }
        }
        $newPaths = @($incoming.Name) + @('Uninstall.exe')
        foreach ($name in $newPaths) {
            $item = Find-InstallEntry $name
            if ($item -and -not $names.Contains($name)) {
                throw 'An unrelated file or directory conflicts with the new package. No files were deleted.'
            }
        }
        foreach ($entry in $owned) {
            Remove-Item -LiteralPath (Join-Path $InstallationDirectory $entry.Name.Replace('/', '\'))
        }
        Remove-Item -LiteralPath $manifestPath
        foreach ($directory in @($directories | Sort-Object -Descending)) {
            if (@(Get-ChildItem -LiteralPath $directory -Force).Count -eq 0) {
                [IO.Directory]::Delete($directory)
            }
        }
        Write-Output 'Verified old payload files were removed. Unrelated files and all external application data were preserved.'
    }
    'RecordInstall' {
        Assert-Stopped
        $manifestPath = Join-Path $InstallationDirectory 'package-manifest.json'
        $owned = @(Get-PackageFiles $manifestPath)
        if ($owned.Count -ge 8192) { throw 'The package has no room for its uninstaller record.' }
        foreach ($entry in $owned) { Assert-InstalledFile $entry | Out-Null }
        $uninstaller = Find-InstallEntry 'Uninstall.exe'
        if (-not $uninstaller -or $uninstaller.PSIsContainer) {
            throw 'The new uninstaller must be a regular file.'
        }
        $hash = (Get-FileHash -LiteralPath $uninstaller.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
        $manifest.files | Add-Member -MemberType NoteProperty -Name 'Uninstall.exe' -Value $hash -ErrorAction Stop
        [IO.File]::WriteAllText($manifestPath, ($manifest | ConvertTo-Json -Depth 10))
    }
    'Install' {
        if ([IntPtr]::Size -ne 8) { throw 'Service registration requires 64-bit PowerShell.' }
        Import-Module (Join-Path $PSHOME 'Modules\Microsoft.PowerShell.Security\Microsoft.PowerShell.Security.psd1') -ErrorAction Stop
        if (-not (Test-Path -LiteralPath $Executable -PathType Leaf)) {
            throw 'The installed executable is missing. No service was registered.'
        }
        Assert-Stopped
        foreach ($name in @($Api, $Worker)) {
            if (Get-OwnedService $name) {
                throw "$name is already installed. Inspect it before changing configuration."
            }
        }
        if (-not $Configuration) { $Configuration = Read-Host 'Absolute backend configuration file' }
        if (-not [IO.Path]::IsPathRooted($Configuration) -or $Configuration.Contains('"')) {
            throw 'Use an absolute configuration path without quotation marks.'
        }
        $Configuration = (Get-Item -LiteralPath $Configuration).FullName
        if ((Get-Item -LiteralPath $Configuration).Attributes -band [IO.FileAttributes]::ReparsePoint) {
            throw 'Configuration must not be a reparse point.'
        }
        if (-not $Credential) {
            $Credential = Get-Credential -Message 'Existing dedicated non-administrator local service account'
        }
        if (-not $Credential) { throw 'Service account selection was cancelled.' }
        $user = Get-DedicatedLocalAccount $Credential.UserName
        $account = [System.Management.Automation.PSCredential]::new("$env:COMPUTERNAME\$($user.Name)", $Credential.Password)
        Write-Output 'The account needs Log on as a service and explicit path permissions. Setup does not grant rights, change ACLs, create roots, or start services.'
        foreach ($role in @(@($Api, 'api'), @($Worker, 'worker'))) {
            $command = '"' + $Executable + '" windows-service ' + $role[1] + ' "' + $Configuration + '"'
            $parameters = @{
                Name = $role[0]
                BinaryPathName = $command
                DisplayName = "Domus Obscura $($role[1])"
                StartupType = 'Manual'
                Credential = $account
            }
            if ($role[1] -eq 'worker') { $parameters.DependsOn = @($Api) }
            New-Service @parameters | Out-Null
            Invoke-ServicePolicy $role[0] 'disable'
        }
        Write-Output 'Services are registered but stopped. Review permissions and configuration before the separate Start action. Partial setup is retained if an error occurs.'
    }
    'Start' {
        Assert-ServiceAccounts
        foreach ($name in @($Api, $Worker)) {
            $service = Get-OwnedService $name
            if (-not $service) { throw "Service $name is not installed." }
            if ($service.State -eq 'Stopped') { Invoke-ServiceControl @('start', $name) }
            Wait-ServiceState $name 'Running'
        }
        foreach ($name in @($Api, $Worker)) {
            Invoke-ServicePolicy $name 'enable'
            Invoke-ServiceControl @('config', $name, 'start=', 'auto')
        }
    }
    'Stop' {
        foreach ($name in @($Worker, $Api)) {
            if (Get-OwnedService $name) {
                Invoke-ServiceControl @('config', $name, 'start=', 'demand')
                Invoke-ServicePolicy $name 'disable'
            }
        }
        foreach ($name in @($Worker, $Api)) {
            $service = Get-OwnedService $name
            if ($service -and $service.State -ne 'Stopped') {
                if ($service.State -ne 'Stop Pending') { Invoke-ServiceControl @('stop', $name) }
                Wait-ServiceState $name 'Stopped'
            }
        }
    }
    'Remove' {
        Assert-Stopped
        foreach ($name in @($Worker, $Api)) {
            if (Get-OwnedService $name) { Invoke-ServiceControl @('delete', $name) }
        }
        Write-Output 'Service registrations were removed. Accounts, permissions, configuration, media, and recovery data were preserved.'
    }
}

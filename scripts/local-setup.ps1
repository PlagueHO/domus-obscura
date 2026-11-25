[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string] $Directory,
    [ValidateRange(1024, 65535)]
    [int] $Port = 8443,
    [switch] $UseExistingParent,
    [switch] $SingleUserHttp
)

$ErrorActionPreference = 'Stop'
if (-not $IsWindows -or $PSVersionTable.PSVersion.Major -lt 7) {
    throw 'Use PowerShell 7 on Windows. See docs/local-development.md for manual setup.'
}
$repository = Split-Path -Parent $PSScriptRoot
$executable = Join-Path $repository 'target\debug\domus.exe'
$assets = Join-Path $repository 'frontend\dist-app'
if (-not (Test-Path -LiteralPath $executable) -or
    -not (Test-Path -LiteralPath (Join-Path $assets 'index.html'))) {
    throw 'Run .\dev.ps1 build before local setup.'
}
if (-not [IO.Path]::IsPathFullyQualified($Directory)) {
    throw 'Directory must be an absolute path to a new folder.'
}
$root = [IO.Path]::GetFullPath($Directory)
if ((Test-Path -LiteralPath $root) -and -not $UseExistingParent) {
    throw 'The setup folder already exists. Choose a new folder. Nothing was changed.'
}
$parent = Split-Path -Parent $root
if (-not (Test-Path -LiteralPath $parent -PathType Container)) {
    throw 'The parent folder must already exist.'
}
$checkedParent = if ($UseExistingParent) { $root } else { $parent }
& $executable root-identity $checkedParent | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw 'The setup parent failed native no-follow inspection. Nothing was changed.'
}
$application = Join-Path $root 'application'
$library = Join-Path $root $(if ($UseExistingParent) { 'developer-library' } else { 'library' })
foreach ($target in @($application, $library)) {
    if (Test-Path -LiteralPath $target) {
        throw "Fresh setup target exists: $target. Nothing was changed."
    }
}
if ($UseExistingParent) {
    if (-not (Test-Path -LiteralPath $root -PathType Container)) {
        throw 'The selected existing parent must be a directory.'
    }
} else {
    New-Item -ItemType Directory -Path $root -ErrorAction Stop | Out-Null
}
$identity = [Security.Principal.WindowsIdentity]::GetCurrent().User
$acl = [Security.AccessControl.DirectorySecurity]::new()
$acl.SetOwner($identity)
$acl.SetAccessRuleProtection($true, $false)
$acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new(
    $identity, 'FullControl', 'ContainerInherit,ObjectInherit', 'None', 'Allow'))
if (-not $UseExistingParent) { Set-Acl -LiteralPath $root -AclObject $acl }
New-Item -ItemType Directory -Path $application -ErrorAction Stop | Out-Null
Set-Acl -LiteralPath $application -AclObject $acl

function Invoke-Domus([string[]] $Arguments) {
    & $executable @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "Domus command failed: $($Arguments[0]). The partial setup was kept at $root."
    }
}

Invoke-Domus -Arguments @('init', '--storage-root', $application)
$configurationFile = Join-Path $application 'config.json'
foreach ($directory in @($library, (Join-Path $application 'staging'), (Join-Path $application 'secrets'))) {
    New-Item -ItemType Directory -Path $directory | Out-Null
}

if (-not $SingleUserHttp) {
$key = [Security.Cryptography.ECDsa]::Create([Security.Cryptography.ECCurve+NamedCurves]::nistP256)
try {
    $request = [Security.Cryptography.X509Certificates.CertificateRequest]::new(
        'CN=localhost', $key, [Security.Cryptography.HashAlgorithmName]::SHA256)
    $san = [Security.Cryptography.X509Certificates.SubjectAlternativeNameBuilder]::new()
    $san.AddDnsName('localhost')
    $san.AddIpAddress([Net.IPAddress]::Loopback)
    $request.CertificateExtensions.Add($san.Build())
    $request.CertificateExtensions.Add(
        [Security.Cryptography.X509Certificates.X509BasicConstraintsExtension]::new($false, $false, 0, $true))
    $request.CertificateExtensions.Add(
        [Security.Cryptography.X509Certificates.X509KeyUsageExtension]::new(
            [Security.Cryptography.X509Certificates.X509KeyUsageFlags]::DigitalSignature, $true))
    $eku = [Security.Cryptography.OidCollection]::new()
    $eku.Add([Security.Cryptography.Oid]::new('1.3.6.1.5.5.7.3.1')) | Out-Null
    $request.CertificateExtensions.Add(
        [Security.Cryptography.X509Certificates.X509EnhancedKeyUsageExtension]::new($eku, $false))
    $certificate = $request.CreateSelfSigned([DateTimeOffset]::UtcNow.AddMinutes(-5), [DateTimeOffset]::UtcNow.AddDays(90))
    try {
        [IO.File]::WriteAllText((Join-Path $application 'secrets\certificate.pem'), $certificate.ExportCertificatePem() + "`n")
        [IO.File]::WriteAllText((Join-Path $application 'secrets\private-key.pem'), $key.ExportPkcs8PrivateKeyPem() + "`n")
    } finally {
        $certificate.Dispose()
    }
} finally {
    $key.Dispose()
}
}

$configuration = Get-Content -LiteralPath $configurationFile -Raw | ConvertFrom-Json -AsHashtable
$configuration.environment = if ($SingleUserHttp) { 'single_user' } else { 'development' }
$configuration.system_settings_read_only = $false
$configuration.listen = "127.0.0.1:$Port"
$configuration.origin = if ($SingleUserHttp) { "http://127.0.0.1:$Port" } else { "https://localhost:$Port" }
if (-not $SingleUserHttp) {
    $configuration.certificate_file = Join-Path $application 'secrets\certificate.pem'
    $configuration.private_key_file = Join-Path $application 'secrets\private-key.pem'
}
$configuration.production_worker = @{ queue_capacity = 64; acquisition = $null }
$configuration.browser_assets = $assets
$mediaRoot = Invoke-Domus -Arguments @(
    'storage-root-inspect', $library, 'library', 'main', '1', 'media', ''
) | ConvertFrom-Json -AsHashtable
$stagingRoot = Invoke-Domus -Arguments @(
    'storage-root-inspect', $application, 'application', 'staging', '1', 'staging', 'staging'
) | ConvertFrom-Json -AsHashtable
$configuration.storage.providers += @{ scope = 'library'; boundary = $library }
$configuration.storage.roots += @($mediaRoot, $stagingRoot)
$configuration.storage.selection_revision += 1

# These generated sibling roots use separate provider views. Bind each private/media
# assurance to the complete selection through the authoritative Rust digest.
$selectionFile = Join-Path $application 'secrets\setup-storage-selection.json'
$selectionStream = [IO.File]::Open($selectionFile, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
try {
    try {
        $selectionBytes = [Text.Encoding]::UTF8.GetBytes(
            ($configuration.storage | ConvertTo-Json -Depth 20) + "`n")
        $selectionStream.Write($selectionBytes)
    } finally {
        $selectionStream.Dispose()
    }
    $selectionDigest = Invoke-Domus -Arguments @('storage-selection-digest', $selectionFile)
    if ($selectionDigest -cnotmatch '^[0-9a-f]{64}$') {
        throw 'Domus returned an invalid storage selection digest.'
    }
    foreach ($privateRoot in @($configuration.storage.roots | Where-Object { $_.role -ne 'media' })) {
        $left, $right = if ([string]::CompareOrdinal($privateRoot.root_id, $mediaRoot.root_id) -lt 0) {
            $privateRoot, $mediaRoot
        } else {
            $mediaRoot, $privateRoot
        }
        $configuration.storage.topology_assurances += @{
            left = @{ root_id = $left.root_id; binding_revision = $left.binding_revision }
            right = @{ root_id = $right.root_id; binding_revision = $right.binding_revision }
            selection_digest = $selectionDigest
        }
    }
} finally {
    Remove-Item -LiteralPath $selectionFile
}
[IO.File]::WriteAllText($configurationFile, ($configuration | ConvertTo-Json -Depth 20) + "`n")

Invoke-Domus -Arguments @('location-add', '--host', $configurationFile, $library, 'main')
Invoke-Domus -Arguments @('scan', '--host', $configurationFile, '1')
Invoke-Domus -Arguments @('staging-init', '--host', $configurationFile, (Join-Path $application 'staging'))

$settingsFile = Join-Path $application 'settings.json'
$settings = & $executable settings --host $configurationFile | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw 'Could not read settings.' }
if ($null -eq $settings) {
    # Use the default exported contract, not a separate path/tag policy.
    $defaults = & $executable settings-default
    if ($LASTEXITCODE -ne 0) { throw 'Could not read default settings.' }
    [IO.File]::WriteAllText($settingsFile, ($defaults -join "`n") + "`n")
    Invoke-Domus -Arguments @('settings-save', '--host', $configurationFile, '0', $settingsFile)
}
Write-Host "Created anonymous development setup: $root"
Write-Host "Certificate trust was NOT changed."
Write-Host "Open $($configuration.origin) after starting:"
Write-Host "& '$executable' developer-run '$configurationFile'"
Write-Host 'Live acquisition is NOT enabled. See docs/local-development.md.'

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string] $Directory,
    [ValidateRange(1024, 65535)]
    [int] $Port = 8443
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
if (Test-Path -LiteralPath $root) {
    throw 'The setup folder already exists. Choose a new folder. Nothing was changed.'
}
$parent = Split-Path -Parent $root
if (-not (Test-Path -LiteralPath $parent -PathType Container)) {
    throw 'The parent folder must already exist.'
}
New-Item -ItemType Directory -Path $root -ErrorAction Stop | Out-Null
$identity = [Security.Principal.WindowsIdentity]::GetCurrent().User
$acl = [Security.AccessControl.DirectorySecurity]::new()
$acl.SetOwner($identity)
$acl.SetAccessRuleProtection($true, $false)
$acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new(
    $identity, 'FullControl', 'ContainerInherit,ObjectInherit', 'None', 'Allow'))
Set-Acl -LiteralPath $root -AclObject $acl

function Invoke-Domus([string[]] $Arguments) {
    & $executable @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "Domus command failed: $($Arguments[0]). The partial setup was kept at $root."
    }
}

foreach ($name in @('state', 'library', 'staging')) {
    New-Item -ItemType Directory -Path (Join-Path $root $name) | Out-Null
}
$database = Join-Path $root 'state\domus.db'
Invoke-Domus -Arguments @('init', $database)
Invoke-Domus -Arguments @('location-add', $database, (Join-Path $root 'library'), 'main')
Invoke-Domus -Arguments @('scan', $database, '1')
Invoke-Domus -Arguments @('staging-init', $database, (Join-Path $root 'staging'))

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
        [IO.File]::WriteAllText((Join-Path $root 'certificate.pem'), $certificate.ExportCertificatePem() + "`n")
        [IO.File]::WriteAllText((Join-Path $root 'private-key.pem'), $key.ExportPkcs8PrivateKeyPem() + "`n")
    } finally {
        $certificate.Dispose()
    }
} finally {
    $key.Dispose()
}

$settingsFile = Join-Path $root 'settings.json'
$settings = & $executable settings $database | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw 'Could not read settings.' }
if ($null -eq $settings) {
    # Use the default exported contract, not a separate path/tag policy.
    $defaults = & $executable settings-default
    if ($LASTEXITCODE -ne 0) { throw 'Could not read default settings.' }
    [IO.File]::WriteAllText($settingsFile, ($defaults -join "`n") + "`n")
    Invoke-Domus -Arguments @('settings-save', $database, '0', $settingsFile)
}
$configuration = [ordered]@{
    environment = 'development'
    database = $database
    listen = "127.0.0.1:$Port"
    origin = "https://localhost:$Port"
    certificate_file = Join-Path $root 'certificate.pem'
    private_key_file = Join-Path $root 'private-key.pem'
    browser_assets = $assets
}
$configurationFile = Join-Path $root 'service.local.json'
[IO.File]::WriteAllText($configurationFile, ($configuration | ConvertTo-Json -Depth 8) + "`n")
Write-Host "Created anonymous development setup: $root"
Write-Host "Certificate trust was NOT changed. Certificate valid for 90 days."
Write-Host "Open https://localhost:$Port after starting:"
Write-Host "& '$executable' api-preview '$configurationFile'"
Write-Host 'Live acquisition is NOT enabled. See docs/local-development.md.'

[CmdletBinding()]
param(
    [string] $Image = 'domus-obscura:candidate'
)

$ErrorActionPreference = 'Stop'
$run = [Guid]::NewGuid().ToString('N')
$volume = "domus-storage-prototype-$run"
$containers = [System.Collections.Generic.List[string]]::new()
$createdVolume = $false

function Invoke-Docker {
    param([string[]] $Arguments)
    $output = & docker @Arguments 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "docker $($Arguments -join ' ') failed: $($output -join "`n")"
    }
    return $output
}

function New-Probe {
    $arguments = @(
        'create', '--pull', 'never', '--network', 'none',
        '--read-only', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges',
        '--label', "io.domus.storage-prototype=$run",
        '--mount', "type=volume,source=$volume,target=/fixture,readonly",
        '--mount', "type=volume,source=$volume,volume-subpath=payload,target=/fixture/media-alias,readonly",
        '--mount', "type=volume,source=$volume,volume-subpath=payload/staging,target=/fixture/staging-alias,readonly",
        '--entrypoint', '/bin/sh', $Image, '-c', 'echo ready; sleep 300'
    )
    $id = ((Invoke-Docker -Arguments $arguments) -join "`n").Trim()
    if ($id -notmatch '^[a-f0-9]{64}$') {
        throw "Unexpected generated container ID: $id"
    }
    $containers.Add($id)
    Invoke-Docker -Arguments @('start', $id) | Out-Null
    return $id
}

function Get-Identity {
    param([string] $Container, [string] $Path)
    $identity = ((Invoke-Docker -Arguments @(
        'exec', $Container, '/opt/domus-obscura/domus', 'root-identity', $Path
    )) -join "`n").Trim()
    if ($identity -notmatch '^unix:[a-f0-9]{16}:[a-f0-9]{16}$') {
        throw "Unexpected native identity: $identity"
    }
    return $identity
}

function Get-Probe {
    param([string] $Container)
    return [ordered]@{
        container = $Container
        namespace_root = Get-Identity $Container '/'
        boundary = Get-Identity $Container '/fixture'
        media = Get-Identity $Container '/fixture/media'
        media_alias = Get-Identity $Container '/fixture/media-alias'
        staging_alias = Get-Identity $Container '/fixture/staging-alias'
        staging_through_media = Get-Identity $Container '/fixture/media-alias/staging'
    }
}

try {
    $imageDetails = ((Invoke-Docker -Arguments @('image', 'inspect', $Image)) -join "`n" | ConvertFrom-Json)[0]
    Invoke-Docker -Arguments @('volume', 'create', '--label', "io.domus.storage-prototype=$run", $volume) | Out-Null
    $createdVolume = $true
    Invoke-Docker -Arguments @(
        'run', '--rm', '--pull', 'never', '--network', 'none', '--read-only',
        '--user', '0:0', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges',
        '--mount', "type=volume,source=$volume,target=/fixture",
        '--entrypoint', '/bin/sh', $Image, '-c',
        'mkdir -p /fixture/media /fixture/payload/staging /fixture/media-alias /fixture/staging-alias'
    ) | Out-Null
    $first = New-Probe
    $second = New-Probe
    $a = Get-Probe $first
    $b = Get-Probe $second
    if ($a.namespace_root -eq $b.namespace_root) {
        throw 'The concurrent containers did not reproduce different namespace-root identities.'
    }
    if ($a.boundary -ne $b.boundary -or $a.media -ne $b.media) {
        throw 'Unchanged retained storage did not have matching identities.'
    }
    Invoke-Docker -Arguments @('rm', '--force', $first) | Out-Null
    $containers.Remove($first) | Out-Null
    $third = New-Probe
    $c = Get-Probe $third
    if ($a.boundary -ne $c.boundary -or $a.media -ne $c.media) {
        throw 'Retained identities changed after container recreation.'
    }
    if ($a.staging_alias -ne $a.staging_through_media) {
        throw 'The generated nested alias did not name the same native directory.'
    }
    $mediaChain = @($a.boundary, $a.media_alias)
    $stagingChain = @($a.boundary, $a.staging_alias)
    $overlapDetected = $mediaChain.Contains($a.staging_alias) -or $stagingChain.Contains($a.media_alias)
    if ($overlapDetected) {
        throw 'The expected hidden-ancestry counterexample was not reproduced.'
    }
    [ordered]@{
        result = 'rejected_unsafe_candidate'
        fixture_run = $run
        explanation = 'Boundary-relative ancestry equality survives recreation, but ancestry membership misses a proven media/staging alias. Production adoption is blocked.'
        image_id = $imageDetails.Id
        image_labels = $imageDetails.Config.Labels
        evidence_scope = 'Existing image root-identity command on generated volumes only. Not current-checkout production integration or Linux-host acceptance.'
        concurrent = @($a, $b)
        recreated = $c
        candidate_media_chain = $mediaChain
        candidate_staging_chain = $stagingChain
        candidate_alias_overlap_detected = $overlapDetected
        actual_alias_overlap_confirmed = $true
    } | ConvertTo-Json -Depth 8
}
finally {
    $cleanupErrors = @()
    foreach ($container in $containers) {
        try {
            Invoke-Docker -Arguments @('rm', '--force', $container) | Out-Null
        }
        catch {
            $cleanupErrors += $_.Exception.Message
        }
    }
    if ($createdVolume) {
        try {
            Invoke-Docker -Arguments @('volume', 'rm', $volume) | Out-Null
        }
        catch {
            $cleanupErrors += $_.Exception.Message
        }
    }
    if ($cleanupErrors.Count -gt 0) {
        throw "Generated fixture cleanup failed: $($cleanupErrors -join '; ')"
    }
}

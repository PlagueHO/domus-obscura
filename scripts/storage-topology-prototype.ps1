[CmdletBinding()]
param(
    [string] $CompilerImage
)

$ErrorActionPreference = 'Stop'
$run = [Guid]::NewGuid().ToString('N')
$volume = "domus-storage-topology-$run"
$containers = [System.Collections.Generic.List[string]]::new()
$createdVolume = $false
$workspace = Split-Path -Parent $PSScriptRoot
$pin = Select-String -LiteralPath (Join-Path $workspace 'rust-toolchain.toml') -Pattern '^channel\s*=\s*"(\d+\.\d+\.\d+)"\s*$'
if (@($pin).Count -ne 1) {
    throw 'The prototype requires an exact Rust version in rust-toolchain.toml.'
}
$requiredVersion = $pin.Matches[0].Groups[1].Value
$paths = @(
    '/', '/fixture', '/fixture/private', '/fixture/private/staging',
    '/fixture/media', '/fixture/media-extra', '/fixture/media-alias',
    '/fixture/staging-alias', '/fixture/media-alias/staging',
    '/fixture/payload', '/fixture/mounted-media',
    '/fixture/mounted-media/injected', '/proc'
)

function Invoke-Docker {
    param([string[]] $Arguments)
    $output = & docker @Arguments 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "docker $($Arguments -join ' ') failed: $($output -join "`n")"
    }
    return $output
}

function New-Container {
    param([string[]] $Arguments)
    $id = ((Invoke-Docker -Arguments (@(
        'create', '--pull', 'never', '--network', 'none', '--read-only',
        '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges',
        '--label', "io.domus.storage-topology=$run"
    ) + $Arguments)) -join "`n").Trim()
    if ($id -notmatch '^[a-f0-9]{64}$') {
        throw "Unexpected generated container ID: $id"
    }
    $containers.Add($id)
    return $id
}

function New-Probe {
    $id = New-Container -Arguments @(
        '--user', '1000:1000',
        '--mount', "type=volume,source=$volume,target=/fixture,readonly",
        '--mount', "type=volume,source=$volume,volume-subpath=payload,target=/fixture/media-alias,readonly",
        '--mount', "type=volume,source=$volume,volume-subpath=payload/staging,target=/fixture/staging-alias,readonly",
        '--mount', "type=volume,source=$volume,volume-subpath=payload/staging,target=/fixture/mounted-media,readonly",
        '--mount', "type=volume,source=$volume,volume-subpath=private,target=/fixture/mounted-media/injected,readonly",
        '--entrypoint', '/bin/sh', $CompilerImage, '-c', 'echo ready; sleep 300'
    )
    Invoke-Docker -Arguments @('start', $id) | Out-Null
    Invoke-Docker -Arguments @('exec', $id, '/bin/sh', '-c', 'test -x /fixture/tools/probe') | Out-Null
    return $id
}

function Read-Probe {
    param([string] $Container)
    return ((Invoke-Docker -Arguments (@('exec', $Container, '/fixture/tools/probe') + $paths)) -join "`n" | ConvertFrom-Json)
}

function Assert-Relationship {
    param($Probe, [int] $Left, [int] $Right, [string] $Expected)
    $relationship = @($Probe.relationships | Where-Object { $_.left -eq $Left -and $_.right -eq $Right })
    if ($relationship.Count -ne 1 -or $relationship[0].result -ne $Expected) {
        throw "Unexpected relationship for $($paths[$Left]) and $($paths[$Right]); expected $Expected."
    }
}

function Assert-Probe {
    param($Probe)
    if (@($Probe.roots).Count -ne $paths.Count) {
        throw 'The probe did not return every requested root.'
    }
    for ($index = 0; $index -lt $paths.Count; $index++) {
        if ($Probe.roots[$index].path -ne $paths[$index] -or $Probe.roots[$index].identity -notmatch '^unix:[a-f0-9]{16}:[a-f0-9]{16}$') {
            throw 'The probe returned a different root or invalid native identity.'
        }
    }
    Assert-Relationship $Probe 2 4 'Separated'
    Assert-Relationship $Probe 4 5 'Separated'
    Assert-Relationship $Probe 2 3 'Overlapping'
    Assert-Relationship $Probe 6 7 'Overlapping'
    Assert-Relationship $Probe 7 6 'Overlapping'
    Assert-Relationship $Probe 7 8 'Overlapping'
    Assert-Relationship $Probe 6 9 'Overlapping'
    Assert-Relationship $Probe 2 10 'UnknownDescendantMount'
    Assert-Relationship $Probe 10 2 'UnknownDescendantMount'
    Assert-Relationship $Probe 2 11 'Overlapping'
    Assert-Relationship $Probe 2 12 'UnknownFilesystem'
    if ($Probe.roots[7].identity -ne $Probe.roots[8].identity -or $Probe.roots[2].identity -ne $Probe.roots[11].identity) {
        throw 'The expected real aliases do not name the same native directories.'
    }
    $mediaChain = @($Probe.roots[6].candidate_binding)
    $stagingChain = @($Probe.roots[7].candidate_binding)
    if ($mediaChain.Contains($Probe.roots[7].identity) -or $stagingChain.Contains($Probe.roots[6].identity)) {
        throw 'The original hidden-ancestry counterexample was not reproduced.'
    }
}

try {
    if (-not $CompilerImage) {
        $localImages = @(Invoke-Docker -Arguments @('image', 'ls', '--quiet', '--no-trunc', 'rust') | Sort-Object -Unique)
        foreach ($localImage in $localImages) {
            $candidate = ((Invoke-Docker -Arguments @('image', 'inspect', $localImage)) -join "`n" | ConvertFrom-Json)[0]
            if ($candidate.Os -eq 'linux' -and $candidate.Architecture -eq 'amd64' -and @($candidate.Config.Env).Contains("RUST_VERSION=$requiredVersion")) {
                $CompilerImage = $candidate.Id
                break
            }
        }
        if (-not $CompilerImage) {
            throw "An already installed Linux x86_64 Rust $requiredVersion SDK image is required. No image was pulled."
        }
    }
    $image = ((Invoke-Docker -Arguments @('image', 'inspect', $CompilerImage)) -join "`n" | ConvertFrom-Json)[0]
    if ($image.Os -ne 'linux' -or $image.Architecture -ne 'amd64' -or -not @($image.Config.Env).Contains("RUST_VERSION=$requiredVersion")) {
        throw "The selected image must be an existing Linux x86_64 Rust $requiredVersion SDK."
    }
    Invoke-Docker -Arguments @('volume', 'create', '--label', "io.domus.storage-topology=$run", $volume) | Out-Null
    $createdVolume = $true
    $setup = New-Container -Arguments @(
        '--mount', "type=volume,source=$volume,target=/fixture",
        '--entrypoint', '/bin/sh', $CompilerImage, '-c',
        'mkdir -p /fixture/tools/scripts /fixture/tools/crates/domus/src/infrastructure/filesystem /fixture/tools/tmp /fixture/private/staging /fixture/media /fixture/media-extra /fixture/payload/staging/injected /fixture/media-alias /fixture/staging-alias /fixture/mounted-media && ln -s private /fixture/linked-private'
    )
    Invoke-Docker -Arguments @('start', '--attach', $setup) | Out-Null
    Invoke-Docker -Arguments @('cp', (Join-Path $workspace 'scripts\storage-topology-prototype.rs'), "${setup}:/fixture/tools/scripts/storage-topology-prototype.rs") | Out-Null
    Invoke-Docker -Arguments @('cp', (Join-Path $workspace 'crates\domus\src\infrastructure\filesystem\topology_prototype.rs'), "${setup}:/fixture/tools/crates/domus/src/infrastructure/filesystem/topology_prototype.rs") | Out-Null
    $builder = New-Container -Arguments @(
        '--env', 'TMPDIR=/fixture/tools/tmp',
        '--mount', "type=volume,source=$volume,target=/fixture",
        '--entrypoint', '/bin/sh', $CompilerImage, '-c',
        'version=$(rustc --version); printf "%s\n" "$version"; case "$version" in "rustc $1 "*) ;; *) echo "Compiler does not match the repository pin" >&2; exit 1 ;; esac; rustc --edition 2024 --deny warnings --test /fixture/tools/scripts/storage-topology-prototype.rs -o /fixture/tools/tests && /fixture/tools/tests && rustc --edition 2024 --deny warnings /fixture/tools/scripts/storage-topology-prototype.rs -o /fixture/tools/probe',
        'prototype-build', $requiredVersion
    )
    $buildOutput = (Invoke-Docker -Arguments @('start', '--attach', $builder)) -join "`n"
    $builderStatus = ((Invoke-Docker -Arguments @('inspect', '--format', '{{.State.ExitCode}}', $builder)) -join "`n").Trim()
    if ($builderStatus -ne '0') {
        throw "The isolated compiler/tests failed: $buildOutput"
    }
    $first = New-Probe
    $second = New-Probe
    $a = Read-Probe $first
    $b = Read-Probe $second
    Assert-Probe $a
    Assert-Probe $b
    $child = (Invoke-Docker -Arguments @(
        'exec', $first, '/fixture/tools/probe', '--child', '/fixture/private', 'staging'
    )) -join "`n" | ConvertFrom-Json
    if ($child.child_identity -ne $a.roots[3].identity) {
        throw 'The ordinary retained-handle child walk returned a different directory.'
    }
    $crossing = & docker exec $first /fixture/tools/probe --child /fixture/mounted-media injected 2>&1
    if ($LASTEXITCODE -eq 0 -or ($crossing -join "`n") -notmatch 'descendant mount crossing rejected') {
        throw 'The child walk did not explicitly reject the real descendant bind mount.'
    }
    if ($a.roots[0].identity -eq $b.roots[0].identity) {
        throw 'The concurrent containers did not have different namespace-root identities.'
    }
    foreach ($invalid in @('/fixture/missing', '/fixture/linked-private', '/fixture/../fixture/private')) {
        $failure = & docker exec $first /fixture/tools/probe $invalid 2>&1
        if ($LASTEXITCODE -eq 0) {
            throw "An invalid root was accepted: $invalid"
        }
        if (($failure -join "`n") -notmatch 'Error:') {
            throw "An invalid root failed without an explicit diagnostic: $invalid"
        }
    }
    Invoke-Docker -Arguments @('rm', '--force', $first) | Out-Null
    $containers.Remove($first) | Out-Null
    $third = New-Probe
    $c = Read-Probe $third
    Assert-Probe $c
    for ($index = 1; $index -lt 12; $index++) {
        if ($a.roots[$index].identity -ne $b.roots[$index].identity -or $a.roots[$index].identity -ne $c.roots[$index].identity) {
            throw "Native identity changed across containers: $($paths[$index])"
        }
        $original = @($a.roots[$index].candidate_binding) -join ','
        if ($original -ne (@($b.roots[$index].candidate_binding) -join ',') -or $original -ne (@($c.roots[$index].candidate_binding) -join ',')) {
            throw "Boundary-relative binding changed across containers: $($paths[$index])"
        }
    }
    [ordered]@{
        result = 'restricted_replacement_passed'
        fixture_run = $run
        image_id = $image.Id
        compiler_output = $buildOutput
        source_hashes = [ordered]@{
            probe = (Get-FileHash (Join-Path $workspace 'scripts\storage-topology-prototype.rs') -Algorithm SHA256).Hash
            policy = (Get-FileHash (Join-Path $workspace 'crates\domus\src\infrastructure\filesystem\topology_prototype.rs') -Algorithm SHA256).Hash
            harness = (Get-FileHash $PSCommandPath -Algorithm SHA256).Hash
        }
        evidence_scope = 'Current isolated Rust probe compiled offline in an existing SDK image. Windows-host Linux Docker, generated local volume, same-ext4 relationships only. Not production integration, native Linux-host or Windows-container acceptance.'
        child_walk = 'Ordinary child identity matched; real descendant bind mount rejected before returning a child handle.'
        unsupported = @('other or different filesystems', 'non-ASCII coordinates', 'descendant mounts for separation', 'host topology changes during effects', 'native Linux-host and Windows containers not exercised')
        concurrent = @($a, $b)
        recreated = $c
    } | ConvertTo-Json -Depth 10
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

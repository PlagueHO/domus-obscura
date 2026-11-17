[CmdletBinding()]
param(
    [switch] $AllowPackageDownload,
    [switch] $AllowCifsMount,
    [switch] $AllowAutomaticCifsModuleLoad,
    [switch] $AllowWitnessTools,
    [switch] $AllowServerHook,
    [switch] $AllowAuthenticatedWitness,
    [switch] $AllowReadOnlyOperations,
    [string] $CompilerImage
)

$ErrorActionPreference = 'Stop'
if (-not $AllowPackageDownload) {
    throw 'Separate approval is required. Pass -AllowPackageDownload only after approval to install Debian SMB tools in the disposable container.'
}
if ($AllowAutomaticCifsModuleLoad -and -not $AllowCifsMount) {
    throw 'Automatic CIFS module loading is only valid with explicitly approved CIFS mounts.'
}
if ($AllowServerHook -and -not $AllowWitnessTools) {
    throw 'The separately approved server-hook proof requires -AllowWitnessTools.'
}
if ($AllowAuthenticatedWitness -and (-not $AllowServerHook -or $AllowCifsMount)) {
    throw 'The separately approved authenticated proof requires -AllowServerHook and protocol-only clients.'
}
if ($AllowReadOnlyOperations -and -not $AllowAuthenticatedWitness) {
    throw 'The separately approved read-only operation proof requires -AllowAuthenticatedWitness.'
}
$run = [Guid]::NewGuid().ToString('N')
$volume = "domus-storage-smb-$run"
$network = "domus-storage-smb-$run"
$imageTag = "domus-storage-smb-lab:$run"
$workspace = Split-Path -Parent $PSScriptRoot
$pin = Select-String -LiteralPath (Join-Path $workspace 'rust-toolchain.toml') -Pattern '^channel\s*=\s*"(\d+\.\d+\.\d+)"\s*$'
if (@($pin).Count -ne 1) { throw 'An exact Rust version is required in rust-toolchain.toml.' }
$requiredVersion = $pin.Matches[0].Groups[1].Value
$containers = [System.Collections.Generic.List[string]]::new()
$pipes = [System.Collections.Generic.List[System.Diagnostics.Process]]::new()
$createdVolume = $false
$createdNetwork = $false
$imageId = $null
$secretVolumes = [System.Collections.Generic.List[string]]::new()
$serverSecrets = $null
$clientSecrets = @{}

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
        'create', '--pull', 'never', '--memory', '512m', '--cpus', '1',
        '--pids-limit', '128', '--label', "io.domus.storage-smb=$run"
    ) + $Arguments)) -join "`n").Trim()
    if ($id -notmatch '^[a-f0-9]{64}$') { throw "Unexpected generated container ID: $id" }
    $containers.Add($id)
    return $id
}

function Start-Checked {
    param([string] $Container)
    $output = (Invoke-Docker -Arguments @('start', '--attach', $Container)) -join "`n"
    $status = ((Invoke-Docker -Arguments @('inspect', '--format', '{{.State.ExitCode}}', $Container)) -join "`n").Trim()
    if ($status -ne '0') { throw "Generated command failed: $output" }
    return $output
}

function New-Server {
    param([bool] $WithChildMount = $false, [bool] $WithPrivateAlias = $false)
    $arguments = @(
        '--network', $network, '--network-alias', 'storage-server', '--read-only',
        '--env', "DOMUS_SMB_LAB_RUN=$run",
        '--cap-drop', 'ALL', '--cap-add', 'SETUID', '--cap-add', 'SETGID',
        '--security-opt', 'no-new-privileges',
        '--tmpfs', '/run:rw,nosuid,nodev,size=16m',
        '--tmpfs', '/var/lib/samba:rw,nosuid,nodev,size=16m',
        '--tmpfs', '/var/cache/samba:rw,nosuid,nodev,size=16m',
        '--tmpfs', '/var/log/samba:rw,nosuid,nodev,size=16m',
        '--mount', "type=volume,source=$volume,target=/fixture,readonly"
    )
    if ($WithChildMount) {
        if (-not $AllowServerHook) { throw 'The child-mount proof requires the approved server hook.' }
        $arguments += @(
            '--mount', "type=volume,source=$volume,target=/fixture/private/mounted,volume-subpath=payload/staging,readonly"
        )
    }
    if ($WithPrivateAlias) {
        if (-not $AllowAuthenticatedWitness) { throw 'The authenticated alias proof requires its explicit approval.' }
        $arguments += @(
            '--mount', "type=volume,source=$volume,target=/fixture/private,volume-subpath=payload/staging,readonly"
        )
    }
    if ($AllowAuthenticatedWitness) {
        $arguments += @(
            '--env', 'DOMUS_SMB_AUTH_PROOF=1',
            '--mount', "type=volume,source=$serverSecrets,target=/secrets/witness,readonly"
        )
    }
    if ($AllowReadOnlyOperations) {
        $arguments += @('--env', 'DOMUS_SMB_READ_PROOF=1')
    }
    $serverExecutable = if ($AllowServerHook) { '/opt/domus-samba-source/bin/default/source3/smbd/smbd' } else { '/usr/sbin/smbd' }
    $command = "mkdir -p /var/lib/samba/private /run/samba && exec $serverExecutable --foreground --no-process-group --debug-stdout --configfile=/etc/samba/smb.conf"
    if ($AllowServerHook) {
        $command += " --option='vfs objects=domus_probe' --option='private dir=/var/lib/samba/private' --option='state directory=/var/lib/samba' --option='lock directory=/run/samba' --option='cache directory=/var/cache/samba' --option='pid directory=/run/samba' --option='ncalrpc dir=/run/samba/ncalrpc'"
    }
    if ($AllowAuthenticatedWitness) {
        $command = $command.Replace(' && exec ', ' && cp /secrets/witness/passdb.tdb /var/lib/samba/private/passdb.tdb && cp /secrets/witness/secrets.tdb /var/lib/samba/private/secrets.tdb && exec ')
    }
    $server = New-Container -Arguments ($arguments + @(
        '--entrypoint', '/bin/sh', $imageId, '-c',
        $command
    ))
    Invoke-Docker -Arguments @('start', $server) | Out-Null
    return $server
}

function New-Client {
    param([string] $Principal)
    $arguments = @(
        '--network', $network, '--read-only', '--cap-drop', 'ALL',
        '--env', "DOMUS_SMB_LAB_RUN=$run",
        '--security-opt', 'no-new-privileges',
        '--tmpfs', '/tmp:rw,nosuid,nodev,size=16m',
        '--mount', "type=volume,source=$volume,target=/fixture,readonly"
    )
    if ($AllowReadOnlyOperations) {
        $arguments += @('--env', 'DOMUS_SMB_READ_PROOF=1')
    }
    if ($AllowCifsMount) { $arguments += @('--cap-add', 'SYS_ADMIN') }
    if ($AllowAuthenticatedWitness) {
        if ($Principal -notin @('a', 'b')) { throw 'Select one generated authenticated principal.' }
        $arguments += @(
            '--mount', "type=volume,source=$($clientSecrets[$Principal]),target=/secrets/client,readonly"
        )
    }
    $client = New-Container -Arguments ($arguments + @(
        '--entrypoint', '/bin/sh', $imageId, '-c', 'echo ready; sleep 300'
    ))
    Invoke-Docker -Arguments @('start', $client) | Out-Null
    return $client
}

function New-SecretVolume {
    param([string] $Role)
    $name = "domus-storage-smb-$run-$Role-$($secretVolumes.Count)"
    Invoke-Docker -Arguments @('volume', 'create', '--label', "io.domus.storage-smb=$run", $name) | Out-Null
    $secretVolumes.Add($name)
    return $name
}

function Read-AuthenticatedClient {
    param([string] $Client, [string] $Principal, [string] $OldContext, [string] $OldPin)
    $arguments = @('exec')
    if ($OldContext) { $arguments += @('--env', "DOMUS_SMB_OLD_CONTEXT=$OldContext") }
    if ($OldPin) { $arguments += @('--env', "DOMUS_SMB_OLD_PIN=$OldPin") }
    return ((Invoke-Docker -Arguments ($arguments + @(
        $Client, '/opt/domus-smb/bin/python',
        '/fixture/tools/storage-smb-auth-witness.py', "--client-$Principal"
    ))) -join "`n" | ConvertFrom-Json)
}

function Assert-AuthenticatedIsolation {
    param([string] $Container, [bool] $Server = $false)
    $details = ((Invoke-Docker -Arguments @('inspect', $Container)) -join "`n" | ConvertFrom-Json)[0]
    if ($details.HostConfig.Memory -ne 536870912 -or $details.HostConfig.NanoCpus -ne 1000000000 -or
        $details.HostConfig.PidsLimit -ne 128 -or -not $details.HostConfig.ReadonlyRootfs -or
        $details.HostConfig.NetworkMode -ne $network -or
        @($details.HostConfig.PortBindings.PSObject.Properties).Count -ne 0 -or
        ($null -ne $details.HostConfig.Binds -and @($details.HostConfig.Binds).Count -ne 0) -or
        @($details.Mounts | Where-Object { $_.Type -notin @('volume', 'tmpfs') }).Count -ne 0 -or
        @($details.Mounts | Where-Object { $_.Type -eq 'volume' -and $_.RW }).Count -ne 0) {
        throw 'An authenticated server or client violates the approved resource or mount limits.'
    }
    $expected = if ($Server) { 'SETGID,SETUID' } else { '' }
    $capabilities = @($details.HostConfig.CapAdd | ForEach-Object { $_ -replace '^CAP_', '' } | Sort-Object)
    if (($capabilities -join ',') -ne $expected -or ($details.HostConfig.CapDrop -join ',') -ne 'ALL') {
        throw 'Unexpected authenticated container capabilities.'
    }
    if (-not $Server) {
        if (@($details.Mounts | Where-Object { $_.Destination -eq '/secrets/witness' }).Count -ne 0) {
            throw 'A client received the witness private-key mount.'
        }
        Invoke-Docker -Arguments @('exec', $Container, 'test', '!', '-e', '/secrets/witness/key.pem') | Out-Null
    }
}

function Assert-AuthenticatedNative {
    param($Result, $Native, $Relationships)
    if ($AllowReadOnlyOperations -and (
        $Result.read_operations.read_chunks -ne 34 -or
        $Result.read_operations.complete_files.'read-empty'.bytes -ne 0 -or
        $Result.read_operations.complete_files.'read-tail'.bytes -ne 32785 -or
        $Result.read_operations.complete_files.'read-full'.bytes -ne 1048576 -or
        @($Result.read_operations.native_refusals).Count -ne 16 -or
        $Result.read_operations.tampered_read_refusals -ne 2 -or
        $Result.read_operations.fixture_file_bound -ne 1048576)) {
        throw 'Authenticated read-only operation evidence is incomplete.'
    }
    if ($Result.result -ne 'authenticated_signed_native_evidence_measured' -or
        $Result.decoder_and_server_refusals -ne 19 -or
        $Result.transport.encrypted_sent -lt 1 -or $Result.transport.verified_decrypted -lt 1) {
        throw 'Authenticated transport/signature/refusal evidence is incomplete.'
    }
    $requiredRefusals = @(
        'foreign_open_context', 'foreign_tree_context', 'foreign_session_context',
        'actual_signed_plaintext_witness', 'closed_open', 'disconnected_context'
    )
    if ($Result.restarted_context_checked) { $requiredRefusals += 'restarted_context' }
    if ($Result.principal -eq 'domus_a') { $requiredRefusals += 'expired_context' }
    if (($Result.wire_refusals -join ',') -ne ($requiredRefusals -join ',') -or
        $Result.actual_plaintext_requests -ne 1 -or
        ($Result.principal -eq 'domus_a' -and -not $Result.expiry_and_fresh_context)) {
        throw 'Actual plaintext, foreign, closed or expired context acceptance is incomplete.'
    }
    if ($Result.principal -eq 'domus_a' -and
        (($Result.authentication_refusals.case -join ',') -ne
            'bad_password,guest_account,null_account,ntlmv1,dialect_downgrade')) {
        throw 'Actual server credential, guest, NTLMv1 or dialect refusals are incomplete.'
    }
    foreach ($observation in $Result.observations.PSObject.Properties) {
        foreach ($role in $observation.Value.roots.PSObject.Properties) {
            $expected = $Native.roots.PSObject.Properties[$role.Name].Value
            if ($role.Value.identity -ne $expected.native_identity -or
                $role.Value.filesystem_uuid -ne $expected.filesystem_uuid -or
                ($role.Value.ancestry -join ',') -ne ($expected.ancestry -join ',')) {
                throw 'Signed native identity/UUID/ancestry differs from the independent collector.'
            }
        }
        $mask = if ($Result.principal -eq 'domus_a') { 15 } else { 3 }
        foreach ($pair in $Relationships.relationships) {
            if (($mask -band (1 -shl $pair.left)) -and ($mask -band (1 -shl $pair.right))) {
                $expected = switch ($pair.result) {
                    'Overlapping' { 0 }
                    'Separated' { 1 }
                    'UnknownFilesystem' { 2 }
                    'UnknownDescendantMount' { 3 }
                    default { throw 'Unexpected independent native relationship.' }
                }
                if ($observation.Value.relationships[$pair.left * 4 + $pair.right] -ne $expected) {
                    throw 'Signed relationship differs from the independent Rust collector.'
                }
            }
        }
    }
    if ($Result.principal -eq 'domus_b' -and @($Result.denied_roles).Count -ne 3) {
        throw 'The restricted generated user did not receive every role refusal.'
    }
}

function Initialize-AuthenticatedSecrets {
    $script:serverSecrets = New-SecretVolume 'witness-secret'
    $script:clientSecrets = @{ a = (New-SecretVolume 'client-a'); b = (New-SecretVolume 'client-b') }
    $setup = New-Container -Arguments @(
        '--network', 'none', '--read-only', '--cap-drop', 'ALL',
        '--env', "DOMUS_SMB_LAB_RUN=$run",
        '--tmpfs', '/run:rw,nosuid,nodev,size=16m',
        '--tmpfs', '/var/lib/samba:rw,nosuid,nodev,size=16m',
        '--tmpfs', '/var/cache/samba:rw,nosuid,nodev,size=16m',
        '--mount', "type=volume,source=$serverSecrets,target=/secrets/witness",
        '--mount', "type=volume,source=$($clientSecrets.a),target=/secrets/client-a",
        '--mount', "type=volume,source=$($clientSecrets.b),target=/secrets/client-b",
        '--mount', "type=volume,source=$volume,target=/fixture,readonly",
        '--entrypoint', '/bin/sh', $imageId, '-c',
        'mkdir -p /run/samba /var/lib/samba/private && /opt/domus-smb/bin/python /fixture/tools/storage-smb-auth-witness.py --setup'
    )
    return (Start-Checked $setup | ConvertFrom-Json)
}

function Measure-AuthenticatedReplacement {
    param([string] $Server, [string] $Client)
    $nativePipe = Start-LabPipe $Server '--native'
    $native = Read-LabPipe $nativePipe
    $held = Start-LabPipe $Client '--held-a'
    $opened = Read-LabPipe $held
    $initial = $opened.observation.roots.'private-alias'.identity
    if ($initial -ne $native.roots.'private-alias'.native_identity) {
        throw 'The authenticated retained root differs from the independent native open.'
    }
    $mutated = $false
    try {
        $mutated = $true
        Invoke-FixtureMutation 'test ! -e /fixture/private-auth-held; test ! -e /fixture/private-auth-replacement; mkdir /fixture/private-auth-replacement; cp /fixture/private/sentinel /fixture/private-auth-replacement/sentinel; mv /fixture/private /fixture/private-auth-held; mv /fixture/private-auth-replacement /fixture/private'
        if ($AllowReadOnlyOperations) {
            $readRefused = Send-LabPipe $held 'read-observed'
            if ($readRefused.phase -ne 'authenticated_read_root_refused' -or
                $readRefused.bytes -ne 0) {
                throw 'An authenticated read used a replaced native parent.'
            }
        }
        $refused = Send-LabPipe $held 'observe'
        if ($refused.phase -ne 'authenticated_replaced_root_refused' -or
            @($refused.PSObject.Properties.Name).Contains('observation')) {
            throw 'A changed root returned authenticated native evidence.'
        }
        $fresh = Send-LabPipe $held 'fresh'
        $freshNativePipe = Start-LabPipe $Server '--native'
        $freshNative = Read-LabPipe $freshNativePipe
        Close-LabPipe $freshNativePipe
        if ($fresh.observation.roots.'private-alias'.identity -eq $initial -or
            $fresh.observation.roots.'private-alias'.identity -ne $freshNative.roots.'private-alias'.native_identity) {
            throw 'The fresh authenticated open did not identify the actual replacement.'
        }
    }
    finally {
        if ($mutated) {
            Invoke-FixtureMutation 'if test -d /fixture/private-auth-held; then if test -d /fixture/private; then test ! -e /fixture/private-auth-replacement; mv /fixture/private /fixture/private-auth-replacement; fi; mv /fixture/private-auth-held /fixture/private; fi; if test -d /fixture/private-auth-replacement; then rm -f /fixture/private-auth-replacement/sentinel; rmdir /fixture/private-auth-replacement; fi'
        }
    }
    $stillRefused = Send-LabPipe $held 'observe'
    $restored = Send-LabPipe $held 'fresh'
    $restoredNative = Send-LabPipe $nativePipe 'observe'
    Close-LabPipe $held
    Close-LabPipe $nativePipe
    if ($stillRefused.phase -ne 'authenticated_replaced_root_refused' -or
        $restored.phase -ne 'authenticated_fresh' -or
        $restored.observation.context -eq $opened.observation.context -or
        $restored.observation.roots.'private-alias'.identity -ne $initial -or
        -not $restoredNative.roots.'private-alias'.path_binding_matches) {
        throw 'The changed context revived or a fresh context did not identify the restored original.'
    }
    return [ordered]@{
        original = $opened; refused = $refused; replacement = $fresh
        invalidated_after_restore = $stillRefused; restored_fresh_context = $restored
        read_refusal = if ($AllowReadOnlyOperations) { $readRefused } else { $null }
    }
}

function Measure-ReadChildReplacement {
    param([string] $Client)
    $held = Start-LabPipe $Client '--held-a'
    $opened = Read-LabPipe $held
    try {
        Invoke-FixtureMutation 'test ! -e /fixture/private/read-sentinel-held; mv /fixture/private/sentinel /fixture/private/read-sentinel-held; printf "replacement child\n" > /fixture/private/sentinel'
        $refused = Send-LabPipe $held 'read-observed'
        if ($refused.phase -ne 'authenticated_read_observed' -or
            $refused.result -ne 'signed_native_refusal' -or
            $refused.errno -ne 116 -or $refused.bytes -ne 0) {
            throw 'The observed child replacement returned read bytes.'
        }
    }
    finally {
        Invoke-FixtureMutation 'if test -f /fixture/private/read-sentinel-held; then rm -f /fixture/private/sentinel; mv /fixture/private/read-sentinel-held /fixture/private/sentinel; fi'
        Close-LabPipe $held
    }
    return [ordered]@{ original = $opened; refused = $refused }
}

function Measure-AuthenticatedProof {
    $imageCheck = New-Container -Arguments @(
        '--network', 'none', '--read-only', '--cap-drop', 'ALL',
        '--entrypoint', '/bin/sh', $imageId, '-eu', '-c',
        'test ! -e /secrets/witness/key.pem; test ! -e /secrets/client/credential'
    )
    Start-Checked $imageCheck | Out-Null
    $publicSetup = Initialize-AuthenticatedSecrets
    $server = New-Server
    $a = New-Client 'a'
    $b = New-Client 'b'
    Assert-AuthenticatedIsolation $server $true
    Assert-AuthenticatedIsolation $a
    Assert-AuthenticatedIsolation $b
    $nativePipe = Start-LabPipe $server '--native'
    $native = Read-LabPipe $nativePipe
    Close-LabPipe $nativePipe
    $relationships = ((Invoke-Docker -Arguments @(
        'exec', $server, '/fixture/tools/probe',
        '/fixture/payload', '/fixture/payload', '/fixture/payload/staging', '/fixture/private'
    )) -join "`n" | ConvertFrom-Json)
    $firstA = Read-AuthenticatedClient $a 'a'
    $firstB = Read-AuthenticatedClient $b 'b'
    Assert-AuthenticatedNative $firstA $native $relationships
    Assert-AuthenticatedNative $firstB $native $relationships
    $nativePipe = Start-LabPipe $server '--native'
    $afterQueries = Read-LabPipe $nativePipe
    Close-LabPipe $nativePipe
    foreach ($role in $native.roots.PSObject.Properties) {
        if ($role.Value.metadata_digest -ne $afterQueries.roots.PSObject.Properties[$role.Name].Value.metadata_digest) {
            throw 'Ordinary authenticated queries changed measured native timestamps or extended attributes.'
        }
    }
    foreach ($client in @($a, $b)) {
        $details = ((Invoke-Docker -Arguments @('inspect', $client)) -join "`n" | ConvertFrom-Json)[0]
        if (@($details.Mounts | Where-Object { $_.Name -eq $serverSecrets }).Count -ne 0) {
            throw 'A client received the server private-key volume.'
        }
        $absent = (Invoke-Docker -Arguments @('exec', $client, 'test', '!', '-e', '/secrets/witness/key.pem'))
        $absent | Out-Null
    }
    Invoke-Docker -Arguments @('stop', '--time', '3', $server) | Out-Null
    Invoke-Docker -Arguments @('rm', $server) | Out-Null
    $containers.Remove($server) | Out-Null
    $readMount = $null
    if ($AllowReadOnlyOperations) {
        $server = New-Server $true
        $held = Start-LabPipe $a '--held-a'
        [void](Read-LabPipe $held)
        $readMount = Send-LabPipe $held 'read-mount'
        Close-LabPipe $held
        if ($readMount.phase -ne 'authenticated_read_observed' -or
            $readMount.result -ne 'signed_native_refusal' -or
            $readMount.errno -ne 18 -or $readMount.bytes -ne 0) {
            throw 'The authenticated operation accepted a native child mount crossing.'
        }
        Invoke-Docker -Arguments @('stop', '--time', '3', $server) | Out-Null
        Invoke-Docker -Arguments @('rm', $server) | Out-Null
        $containers.Remove($server) | Out-Null
    }
    $server = New-Server
    $nativePipe = Start-LabPipe $server '--native'
    $secondNative = Read-LabPipe $nativePipe
    Close-LabPipe $nativePipe
    foreach ($role in $native.roots.PSObject.Properties) {
        $recreated = $secondNative.roots.PSObject.Properties[$role.Name].Value
        if ($role.Value.native_identity -ne $recreated.native_identity -or
            $role.Value.filesystem_uuid -ne $recreated.filesystem_uuid) {
            throw 'The generated native backing identity changed during server recreation.'
        }
    }
    $secondA = Read-AuthenticatedClient $a 'a' $firstA.valid_after_refusals.context
    $secondB = Read-AuthenticatedClient $b 'b' $firstB.valid_after_refusals.context
    Assert-AuthenticatedNative $secondA $secondNative $relationships
    Assert-AuthenticatedNative $secondB $secondNative $relationships
    if (-not $secondA.restarted_context_checked -or -not $secondB.restarted_context_checked) {
        throw 'Old server-generation contexts were not refused on actual new connections.'
    }
    $childReplacement = if ($AllowReadOnlyOperations) { Measure-ReadChildReplacement $a } else { $null }
    $replacement = Measure-AuthenticatedReplacement $server $a
    Invoke-Docker -Arguments @('stop', '--time', '3', $server) | Out-Null
    Invoke-Docker -Arguments @('rm', $server) | Out-Null
    $containers.Remove($server) | Out-Null
    $rotatedSetup = Initialize-AuthenticatedSecrets
    if ($rotatedSetup.public_pin -eq $publicSetup.public_pin) { throw 'The witness pin did not rotate.' }
    $server = New-Server $false $true
    $rotatedA = New-Client 'a'
    $rotatedB = New-Client 'b'
    $nativePipe = Start-LabPipe $server '--native'
    $aliasNative = Read-LabPipe $nativePipe
    Close-LabPipe $nativePipe
    $aliasRelationships = ((Invoke-Docker -Arguments @(
        'exec', $server, '/fixture/tools/probe',
        '/fixture/payload', '/fixture/payload', '/fixture/payload/staging', '/fixture/private'
    )) -join "`n" | ConvertFrom-Json)
    $rotatedResultA = Read-AuthenticatedClient $rotatedA 'a' $secondA.valid_after_refusals.context $publicSetup.public_pin
    $rotatedResultB = Read-AuthenticatedClient $rotatedB 'b' $secondB.valid_after_refusals.context $publicSetup.public_pin
    Assert-AuthenticatedNative $rotatedResultA $aliasNative $aliasRelationships
    Assert-AuthenticatedNative $rotatedResultB $aliasNative $aliasRelationships
    Assert-AuthenticatedIsolation $rotatedA
    Assert-AuthenticatedIsolation $rotatedB
    if (-not $rotatedResultA.old_pin_refused -or -not $rotatedResultB.old_pin_refused -or
        $aliasNative.roots.staging.native_identity -ne $aliasNative.roots.'private-alias'.native_identity -or
        $rotatedResultA.observations.media.relationships[3] -ne 0 -or
        $rotatedResultA.observations.media.relationships[12] -ne 0) {
        throw 'Operator key rotation or the actual hidden nested alias was not verified.'
    }
    Invoke-Docker -Arguments @('stop', '--time', '3', $server) | Out-Null
    Invoke-Docker -Arguments @('rm', $server) | Out-Null
    $containers.Remove($server) | Out-Null
    $missingKeyVolume = New-SecretVolume 'missing-key'
    $keylessSetup = New-Container -Arguments @(
        '--network', 'none', '--read-only', '--cap-drop', 'ALL',
        '--mount', "type=volume,source=$serverSecrets,target=/source,readonly",
        '--mount', "type=volume,source=$missingKeyVolume,target=/destination",
        '--entrypoint', '/bin/sh', $imageId, '-eu', '-c',
        'cp /source/passdb.tdb /destination/passdb.tdb; cp /source/secrets.tdb /destination/secrets.tdb; test ! -e /destination/key.pem'
    )
    Start-Checked $keylessSetup | Out-Null
    $script:serverSecrets = $missingKeyVolume
    $server = New-Server
    $missingKey = ((Invoke-Docker -Arguments @(
        'exec', $rotatedA, '/opt/domus-smb/bin/python',
        '/fixture/tools/storage-smb-auth-witness.py', '--missing-key'
    )) -join "`n" | ConvertFrom-Json)
    $keyLog = (Invoke-Docker -Arguments @('logs', '--tail', '50', $server)) -join "`n"
    if ($missingKey.result -ne 'missing_witness_key_refused' -or
        $keyLog -notmatch 'Authenticated witness refused startup key: 2') {
        throw 'The actual missing-key failure did not stop authenticated native evidence.'
    }
    Assert-AuthenticatedIsolation $server $true
    $details = ((Invoke-Docker -Arguments @('inspect', $server)) -join "`n" | ConvertFrom-Json)[0]
    $networkDetails = ((Invoke-Docker -Arguments @('network', 'inspect', $network)) -join "`n" | ConvertFrom-Json)[0]
    if (-not $networkDetails.Internal -or @($details.HostConfig.PortBindings.PSObject.Properties).Count -ne 0) {
        throw 'Authenticated server isolation differs from the approved lab.'
    }
    return [ordered]@{
        result = 'authenticated_signed_native_witness_measured'
        fixture_run = $run
        public_pin = $publicSetup.public_pin
        clients = @($firstA, $firstB)
        recreated_clients = @($secondA, $secondB)
        recreated_native = $secondNative
        root_replacement = $replacement
        read_child_replacement = $childReplacement
        read_mount_crossing = $readMount
        read_operations_enabled = [bool]$AllowReadOnlyOperations
        rotated_public_pin = $rotatedSetup.public_pin
        rotated_clients = @($rotatedResultA, $rotatedResultB)
        hidden_alias_native = $aliasNative
        hidden_alias_relationships = $aliasRelationships
        missing_key = $missingKey
        independent_native = $native
        native_after_ordinary_queries = $afterQueries
        independent_relationships = $relationships
        private_key_isolation = 'Server-only secret volume; absent from both client namespaces and shared image.'
        versions = $versions
        installation_output = $installation
        source_hashes = [ordered]@{
            harness = (Get-FileHash $PSCommandPath).Hash
            hook = (Get-FileHash (Join-Path $PSScriptRoot 'storage-smb-native-probe.c')).Hash
            auth_hook = (Get-FileHash (Join-Path $PSScriptRoot 'storage-smb-auth-probe.h')).Hash
            auth_policy = (Get-FileHash (Join-Path $PSScriptRoot 'storage-smb-auth-policy.rs')).Hash
            auth_client = (Get-FileHash (Join-Path $PSScriptRoot 'storage-smb-auth-witness.py')).Hash
            auth_config = (Get-FileHash (Join-Path $PSScriptRoot 'storage-smb-auth-lab.conf')).Hash
            builder = (Get-FileHash (Join-Path $PSScriptRoot 'storage-smb-hook-build.sh')).Hash
            native_collector = (Get-FileHash (Join-Path $PSScriptRoot 'storage-smb-witness.py')).Hash
            standalone_probe = (Get-FileHash (Join-Path $PSScriptRoot 'storage-topology-prototype.rs')).Hash
            shared_rule = (Get-FileHash (Join-Path $workspace 'crates\domus\src\infrastructure\filesystem\topology_prototype.rs')).Hash
        }
        limits = 'Generated four-role Linux/ext4 read-only diagnostic only; not production binding or operation acceptance.'
    }
}

function Mount-Exports {
    param([string] $Client)
    $filesystems = (Invoke-Docker -Arguments @('exec', $Client, 'cat', '/proc/filesystems')) -join "`n"
    if ($filesystems -notmatch '(?m)^\s*(?:nodev\s+)?cifs\s*$' -and -not $AllowAutomaticCifsModuleLoad) {
        throw 'CIFS is not registered. Automatic module loading needs separate approval.'
    }
    $resolved = @(Invoke-Docker -Arguments @('exec', $Client, 'getent', 'ahostsv4', 'storage-server'))
    $address = ($resolved[0] -split '\s+')[0]
    if ($address -notmatch '^(\d{1,3}\.){3}\d{1,3}$') {
        throw 'The internal SMB server did not resolve to an IPv4 address.'
    }
    foreach ($export in @('media', 'media-alias', 'staging', 'private-alias')) {
        # Direct mount retains the approved SYS_ADMIN-only boundary. mount.cifs
        # requires an additional capability even for this generated guest share.
        Invoke-Docker -Arguments @(
            'exec', $Client, 'mount', '-i', '-t', 'cifs', "//storage-server/$export", "/mounts/$export",
            '-o', "ip=$address,username=,password=,sec=none,vers=3.1.1,ro,nosuid,nodev,noexec,serverino,actimeo=0,soft"
        ) | Out-Null
    }
}

function Read-Mounted {
    param([string] $Client)
    $probe = ((Invoke-Docker -Arguments @(
        'exec', $Client, '/fixture/tools/probe', '/fixture', '/fixture/private',
        '/mounts/media', '/mounts/staging', '/mounts/media-alias',
        '/mounts/media/staging', '/mounts/private-alias'
    )) -join "`n" | ConvertFrom-Json)
    foreach ($pair in @(@(1, 6), @(2, 3), @(3, 2), @(2, 4), @(3, 5))) {
        $relationship = @($probe.relationships | Where-Object { $_.left -eq $pair[0] -and $_.right -eq $pair[1] })
        if ($relationship.Count -ne 1 -or $relationship[0].result -eq 'Separated') {
            throw 'The prototype granted separation to an unknown or overlapping remote alias.'
        }
    }
    Invoke-Docker -Arguments @(
        'exec', $Client, 'cmp', '/mounts/media/staging/sentinel', '/mounts/staging/sentinel'
    ) | Out-Null
    Invoke-Docker -Arguments @(
        'exec', $Client, 'cmp', '/fixture/private/sentinel', '/mounts/private-alias/sentinel'
    ) | Out-Null
    return $probe
}

function Wait-Server {
    param([string] $Client, [string] $Server)
    for ($attempt = 0; $attempt -lt 15; $attempt++) {
        $output = & docker exec $Client smbclient '//storage-server/staging' '-N' '-m' 'SMB3' '-t' '2' '-c' 'ls' 2>&1
        if ($LASTEXITCODE -eq 0) { return }
        $state = ((Invoke-Docker -Arguments @('inspect', '--format', '{{.State.Running}}', $Server)) -join "`n").Trim()
        if ($state -ne 'true') {
            $log = (Invoke-Docker -Arguments @('logs', '--tail', '30', $Server)) -join "`n"
            throw "Generated SMB server failed to start: $log"
        }
        Start-Sleep -Seconds 1
    }
    $log = (Invoke-Docker -Arguments @('logs', '--tail', '50', $Server)) -join "`n"
    throw "Generated SMB server was not responsive: $($output -join "`n")`nServer: $log"
}

function Read-Exports {
    param([string] $Client)
    $results = [ordered]@{}
    foreach ($export in @('media', 'media-alias', 'staging', 'private-alias')) {
        $relative = if ($export -in @('media', 'media-alias')) { 'staging\sentinel' } else { 'sentinel' }
        $control = if ($export -eq 'private-alias') { '/fixture/private/sentinel' } else { '/fixture/payload/staging/sentinel' }
        $metadata = (Invoke-Docker -Arguments @(
            'exec', $Client, 'smbclient', "//storage-server/$export", '-N', '-m', 'SMB3', '-t', '3',
            '-c', "volume; allinfo $relative; get $relative /tmp/smb-sentinel"
        )) -join "`n"
        Invoke-Docker -Arguments @('exec', $Client, 'cmp', '/tmp/smb-sentinel', $control) | Out-Null
        $results[$export] = $metadata
    }
    return $results
}

function Read-LabPipe {
    param([System.Diagnostics.Process] $Process)
    $read = $Process.StandardOutput.ReadLineAsync()
    if (-not $read.Wait(12000)) { throw 'The generated witness context did not respond within its bound.' }
    $line = $read.GetAwaiter().GetResult()
    if (-not $line -or $line.Length -gt 16384) {
        if ($Process.HasExited) {
            throw "Generated witness failed: $($Process.StandardError.ReadToEnd())"
        }
        throw 'The generated witness returned missing or oversized evidence.'
    }
    return $line | ConvertFrom-Json
}

function Start-LabPipe {
    param([string] $Container, [string] $Mode)
    $start = [System.Diagnostics.ProcessStartInfo]::new('docker')
    $start.UseShellExecute = $false
    $start.RedirectStandardInput = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $collector = if ($Mode -eq '--held-a') { '/fixture/tools/storage-smb-auth-witness.py' } else { '/fixture/tools/storage-smb-witness.py' }
    foreach ($argument in @(
        'exec', '--interactive', $Container, '/opt/domus-smb/bin/python',
        $collector, $Mode
    )) { $start.ArgumentList.Add($argument) }
    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $start
    if (-not $process.Start()) { throw 'Could not start the generated witness context.' }
    $pipes.Add($process)
    return $process
}

function Send-LabPipe {
    param([System.Diagnostics.Process] $Process, [string] $Command)
    $Process.StandardInput.WriteLine($Command)
    $Process.StandardInput.Flush()
    return Read-LabPipe $Process
}

function Assert-WitnessBridge {
    param($Remote, $Native)
    if (@($Remote.roots.PSObject.Properties).Count -ne 4 -or @($Native.roots.PSObject.Properties).Count -ne 4) {
        throw 'The witness did not return every generated root.'
    }
    foreach ($role in @('media', 'media-alias', 'staging', 'private-alias')) {
        $remoteRoot = $Remote.roots.PSObject.Properties[$role].Value
        $nativeRoot = $Native.roots.PSObject.Properties[$role].Value
        if ($remoteRoot.native_identity -notmatch '^unix:[a-f0-9]{16}:[a-f0-9]{16}$' -or
            $remoteRoot.native_identity -ne $nativeRoot.native_identity) {
            throw "The existing SMB open did not match the retained native handle: $role"
        }
    }
}

function Invoke-FixtureMutation {
    param([string] $Command)
    $helper = New-Container -Arguments @(
        '--network', 'none', '--read-only', '--cap-drop', 'ALL',
        '--security-opt', 'no-new-privileges',
        '--mount', "type=volume,source=$volume,target=/fixture",
        '--entrypoint', '/bin/sh', $imageId, '-eu', '-c', $Command
    )
    Start-Checked $helper | Out-Null
}

function Measure-Witness {
    param([string] $Server, [string] $First, [string] $Second, $Backing)
    $nativeProcess = Start-LabPipe $Server '--native'
    $native = Read-LabPipe $nativeProcess
    $roles = @{ 'media' = 2; 'media-alias' = 2; 'staging' = 3; 'private-alias' = 1 }
    foreach ($role in $roles.Keys) {
        if ($native.roots.PSObject.Properties[$role].Value.native_identity -ne $Backing.roots[$roles[$role]].identity) {
            throw 'The retained native context disagreed with the independent Rust collector.'
        }
    }
    $firstProcess = Start-LabPipe $First '--remote'
    $secondProcess = Start-LabPipe $Second '--remote'
    $firstOpen = Read-LabPipe $firstProcess
    $secondOpen = Read-LabPipe $secondProcess
    Assert-WitnessBridge $firstOpen $native
    Assert-WitnessBridge $secondOpen $native
    $afterQueries = Send-LabPipe $nativeProcess 'observe'
    foreach ($role in $roles.Keys) {
        if ($native.roots.PSObject.Properties[$role].Value.metadata_digest -ne
            $afterQueries.roots.PSObject.Properties[$role].Value.metadata_digest) {
            throw 'The Samba metadata hook changed generated directory metadata.'
        }
    }
    $replacementStarted = $false
    try {
        $replacementStarted = $true
        Invoke-FixtureMutation 'test ! -e /fixture/private-held; test ! -e /fixture/private-replacement; mkdir /fixture/private-replacement; cp /fixture/private/sentinel /fixture/private-replacement/sentinel; mv /fixture/private /fixture/private-held; mv /fixture/private-replacement /fixture/private'
        $retainedNative = Send-LabPipe $nativeProcess 'observe'
        $retainedRemote = Send-LabPipe $firstProcess 'observe'
        Assert-WitnessBridge $retainedRemote $retainedNative
        if ($retainedNative.roots.'private-alias'.path_binding_matches -ne $false -or
            $retainedRemote.roots.'private-alias'.native_identity -ne $firstOpen.roots.'private-alias'.native_identity) {
            throw 'Replacement was not rejected while the original remote/native handles retained their identity.'
        }
        $freshProcess = Start-LabPipe $Second '--remote'
        $fresh = Read-LabPipe $freshProcess
        $current = ((Invoke-Docker -Arguments @(
            'exec', $Server, '/fixture/tools/probe', '/fixture', '/fixture/private',
            '/fixture/payload', '/fixture/payload/staging'
        )) -join "`n" | ConvertFrom-Json)
        if ($fresh.roots.'private-alias'.native_identity -ne $current.roots[1].identity -or
            $fresh.roots.'private-alias'.native_identity -eq $firstOpen.roots.'private-alias'.native_identity) {
            throw 'A fresh SMB open did not distinguish the replacement from the retained original.'
        }
        $freshProcess.StandardInput.WriteLine('exit')
        $freshProcess.StandardInput.Flush()
        if (-not $freshProcess.WaitForExit(5000) -or $freshProcess.ExitCode -ne 0) {
            throw 'The fresh generated remote context did not close cleanly.'
        }
    }
    finally {
        if ($replacementStarted) {
            Invoke-FixtureMutation 'if test -d /fixture/private-held; then if test -d /fixture/private; then test ! -e /fixture/private-replacement; mv /fixture/private /fixture/private-replacement; fi; mv /fixture/private-held /fixture/private; fi; if test -d /fixture/private-replacement; then rm -f /fixture/private-replacement/sentinel; rmdir /fixture/private-replacement; fi'
        }
    }
    $restored = Send-LabPipe $nativeProcess 'observe'
    foreach ($role in $roles.Keys) {
        if (-not $restored.roots.PSObject.Properties[$role].Value.path_binding_matches) {
            throw 'The generated original root was not restored exactly.'
        }
    }
    $foreign = Send-LabPipe $firstProcess 'foreign-tree'
    $closed = Send-LabPipe $firstProcess 'closed'
    if ($foreign.rejected -ne $true -or $closed.rejected -ne $true) {
        throw 'The server did not reject invalid existing-open associations.'
    }
    foreach ($process in @($nativeProcess, $firstProcess, $secondProcess)) {
        $process.StandardInput.WriteLine('exit')
        $process.StandardInput.Flush()
        if (-not $process.WaitForExit(5000) -or $process.ExitCode -ne 0) {
            throw 'A generated retained-handle context did not close cleanly.'
        }
    }
    return [ordered]@{
        result = 'retained_samba_native_identity_bridge_measured'
        clients = @($firstOpen, $secondOpen)
        native = $native
        metadata_unchanged = $true
        retained_after_replacement = $retainedRemote
        native_after_replacement = $retainedNative
        fresh_after_replacement = $fresh
        restored_native = $restored
        foreign_tree = $foreign
        closed_open = $closed
        limits = 'Pinned Samba 4.17.12 generated guest fixture only. CREATE_OR_GET_OBJECT_ID is effect-free in this implementation, not a generic read-only operation. Full native device/inode association measured; no stable provider identity, authenticated production transport, cross-provider ancestry or parent-relative effect acceptance.'
    }
}

function Measure-CifsParent {
    param([string] $Client)
    $started = $false
    try {
        $started = $true
        Invoke-FixtureMutation 'test ! -e /fixture/payload/handle-parent; test ! -e /fixture/payload/handle-parent-held; test ! -e /fixture/payload/handle-parent-replacement; mkdir /fixture/payload/handle-parent /fixture/payload/handle-parent-replacement; printf "original-parent\n" > /fixture/payload/handle-parent/sentinel; printf "replacement-parent\n" > /fixture/payload/handle-parent-replacement/sentinel'
        $process = Start-LabPipe $Client '--cifs-parent'
        $original = Read-LabPipe $process
        if ($original.marker -ne "original-parent`n") { throw 'The generated original parent control was not read.' }
        Invoke-FixtureMutation 'mv /fixture/payload/handle-parent /fixture/payload/handle-parent-held; mv /fixture/payload/handle-parent-replacement /fixture/payload/handle-parent'
        $after = Send-LabPipe $process 'observe'
        if ($after.rejected -ne $true -and $after.marker -notin @("original-parent`n", "replacement-parent`n")) {
            throw 'The retained CIFS directory returned neither known generated object.'
        }
        $process.StandardInput.WriteLine('exit')
        $process.StandardInput.Flush()
        if (-not $process.WaitForExit(5000) -or $process.ExitCode -ne 0) {
            throw 'The generated CIFS parent context did not close cleanly.'
        }
        return [ordered]@{
            result = if ($after.rejected -eq $true) {
                'cifs_parent_replacement_rejected'
            } elseif ($after.child_inode -eq $original.child_inode -and $after.marker -eq $original.marker) {
                'retained_cifs_parent_subset_measured'
            } else {
                'cifs_parent_path_retarget_measured'
            }
            before = $original
            after = $after
            authority = 'Generated read-only diagnostic only. A directory descriptor is not sufficient evidence of a retained native server parent. No new media effect is authorized.'
        }
    }
    finally {
        if ($started) {
            Invoke-FixtureMutation 'if test -d /fixture/payload/handle-parent-held; then if test -d /fixture/payload/handle-parent; then test ! -e /fixture/payload/handle-parent-replacement; mv /fixture/payload/handle-parent /fixture/payload/handle-parent-replacement; fi; mv /fixture/payload/handle-parent-held /fixture/payload/handle-parent; fi; for directory in /fixture/payload/handle-parent /fixture/payload/handle-parent-replacement; do if test -d "$directory"; then rm -f "$directory/sentinel"; rmdir "$directory"; fi; done'
        }
    }
}

function Close-LabPipe {
    param([System.Diagnostics.Process] $Process)
    $Process.StandardInput.WriteLine('exit')
    $Process.StandardInput.Flush()
    if (-not $Process.WaitForExit(5000) -or $Process.ExitCode -ne 0) {
        throw 'The generated server-hook context did not close cleanly.'
    }
}

function Wait-NativeParentPause {
    param([string] $Server, [int] $PreviousCount)
    for ($attempt = 0; $attempt -lt 15; $attempt++) {
        $log = (Invoke-Docker -Arguments @('logs', '--tail', '128', $Server)) -join "`n"
        if ([regex]::Matches($log, 'DOMUS_NATIVE_PROBE_PAUSED').Count -gt $PreviousCount) { return }
        Start-Sleep -Milliseconds 200
    }
    throw 'The native server hook did not reach its controlled pause.'
}

function Measure-ServerHook {
    param([string] $Server, [string] $Client, [string] $OtherClient)
    $nativeProcess = Start-LabPipe $Server '--native'
    $native = Read-LabPipe $nativeProcess
    $process = Start-LabPipe $Client '--server-hook'
    $opened = Read-LabPipe $process
    foreach ($role in @('media', 'media-alias', 'staging', 'private-alias')) {
        $actual = $opened.roots.PSObject.Properties[$role].Value
        $expected = $native.roots.PSObject.Properties[$role].Value
        if ($actual.parent_native_identity -ne $expected.native_identity) {
            throw 'The server hook did not borrow the exact registered native parent.'
        }
        if ($role -in @('staging', 'private-alias')) {
            if ($actual.result -ne 'native_child_read' -or
                $actual.child_native_identity -ne $expected.sentinel_identity) {
                throw 'The native child read did not match the independent retained-handle collector.'
            }
        } elseif ($actual.result -ne 'native_refusal' -or $actual.errno -ne 2) {
            throw 'A missing generated child did not produce an explicit native refusal.'
        }
    }
    $afterQueries = Send-LabPipe $nativeProcess 'observe'
    foreach ($role in @('media', 'media-alias', 'staging', 'private-alias')) {
        if ($native.roots.PSObject.Properties[$role].Value.metadata_digest -ne
            $afterQueries.roots.PSObject.Properties[$role].Value.metadata_digest) {
            throw 'The initial read-only hook queries changed generated directory metadata.'
        }
    }
    $malformed = Send-LabPipe $process 'malformed'
    if ($malformed.rejected -ne 14 -or $malformed.valid_after.result -ne 'native_child_read') {
        throw 'The server did not reject every malformed/replayed request and preserve valid sequencing.'
    }
    $metadataOnly = Send-LabPipe $process 'metadata-only'
    if ($metadataOnly.rejected -ne $true) { throw 'Metadata-only access authorized a native child read.' }
    $unrelated = Send-LabPipe $process 'unrelated-parent'
    if ($unrelated.result -ne 'native_refusal' -or $unrelated.errno -ne 116 -or $null -ne $unrelated.marker) {
        throw 'An unrelated native parent was accepted for the registered role.'
    }
    $fixturesStarted = $false
    try {
        $fixturesStarted = $true
        Invoke-FixtureMutation 'test ! -e /fixture/private/denied; test ! -e /fixture/private/link; test ! -e /fixture/private/fifo; test ! -e /fixture/private/large; printf "denied\n" > /fixture/private/denied; chmod 000 /fixture/private/denied; ln -s sentinel /fixture/private/link; mkfifo /fixture/private/fifo; printf "12345678901234567890123456789012345" > /fixture/private/large'
        $files = Send-LabPipe $process 'file-fixtures'
        foreach ($case in @(@('denied', 13), @('link', 40), @('fifo', 22), @('large', 90))) {
            $result = $files.PSObject.Properties[$case[0]].Value
            if ($result.result -ne 'native_refusal' -or $result.errno -ne $case[1] -or $null -ne $result.marker) {
                throw "The native file fixture did not produce its exact refusal: $($case[0])"
            }
        }
    }
    finally {
        if ($fixturesStarted) {
            Invoke-FixtureMutation 'rm -f /fixture/private/denied /fixture/private/link /fixture/private/fifo /fixture/private/large'
        }
    }
    $parentStarted = $false
    try {
        $log = (Invoke-Docker -Arguments @('logs', '--tail', '128', $Server)) -join "`n"
        $submitted = Send-LabPipe $process 'hook-pause'
        if ($submitted.phase -ne 'native_parent_request_submitted') { throw 'The parent probe was not submitted.' }
        Wait-NativeParentPause $Server ([regex]::Matches($log, 'DOMUS_NATIVE_PROBE_PAUSED').Count)
        $parentStarted = $true
        Invoke-FixtureMutation 'test ! -e /fixture/private-probe-held; test ! -e /fixture/private-probe-replacement; test ! -e /fixture/tools/release-parent-probe; mkdir /fixture/private-probe-replacement; printf "replacement-parent\n" > /fixture/private-probe-replacement/sentinel; mv /fixture/private /fixture/private-probe-held; mv /fixture/private-probe-replacement /fixture/private; printf "release\n" > /fixture/tools/release-parent-probe'
        $parentResult = Read-LabPipe $process
        if ($parentResult.result -ne 'native_refusal' -or $parentResult.errno -ne 116 -or
            $parentResult.parent_native_identity -ne $opened.roots.'private-alias'.parent_native_identity -or
            $parentResult.child_native_identity -ne $opened.roots.'private-alias'.child_native_identity -or
            $null -ne $parentResult.marker) {
            throw 'The paused hook retargeted its native parent or failed to reject the changed binding.'
        }
        $freshProcess = Start-LabPipe $OtherClient '--server-hook'
        $fresh = Read-LabPipe $freshProcess
        if ($fresh.roots.'private-alias'.marker -ne "replacement-parent`n" -or
            $fresh.roots.'private-alias'.child_native_identity -eq $opened.roots.'private-alias'.child_native_identity) {
            throw 'A fresh server-hook context did not distinguish the replaced parent.'
        }
        Close-LabPipe $freshProcess
    }
    finally {
        if ($parentStarted) {
            Invoke-FixtureMutation 'if test -d /fixture/private-probe-held; then if test -d /fixture/private; then test ! -e /fixture/private-probe-replacement; mv /fixture/private /fixture/private-probe-replacement; fi; mv /fixture/private-probe-held /fixture/private; fi; if test -d /fixture/private-probe-replacement; then rm -f /fixture/private-probe-replacement/sentinel; rmdir /fixture/private-probe-replacement; fi; rm -f /fixture/tools/release-parent-probe'
        }
    }
    $childStarted = $false
    try {
        $log = (Invoke-Docker -Arguments @('logs', '--tail', '128', $Server)) -join "`n"
        Send-LabPipe $process 'hook-pause' | Out-Null
        Wait-NativeParentPause $Server ([regex]::Matches($log, 'DOMUS_NATIVE_PROBE_PAUSED').Count)
        $childStarted = $true
        Invoke-FixtureMutation 'test ! -e /fixture/private/sentinel-probe-held; test ! -e /fixture/tools/release-parent-probe; mv /fixture/private/sentinel /fixture/private/sentinel-probe-held; printf "replacement-child\n" > /fixture/private/sentinel; printf "release\n" > /fixture/tools/release-parent-probe'
        $childResult = Read-LabPipe $process
        if ($childResult.result -ne 'native_refusal' -or $childResult.errno -ne 116 -or
            $childResult.child_native_identity -eq $opened.roots.'private-alias'.child_native_identity -or
            $null -ne $childResult.marker) {
            throw 'The hook failed to reject a replaced observed child before returning bytes.'
        }
    }
    finally {
        if ($childStarted) {
            Invoke-FixtureMutation 'if test -f /fixture/private/sentinel-probe-held; then rm -f /fixture/private/sentinel; mv /fixture/private/sentinel-probe-held /fixture/private/sentinel; fi; rm -f /fixture/tools/release-parent-probe'
        }
    }
    $restored = Send-LabPipe $process 'observe'
    if ($restored.roots.'private-alias'.child_native_identity -ne $opened.roots.'private-alias'.child_native_identity -or
        $restored.roots.'private-alias'.marker -ne $opened.roots.'private-alias'.marker) {
        throw 'The original generated child was not restored exactly.'
    }
    $foreign = Send-LabPipe $process 'foreign-tree'
    $closed = Send-LabPipe $process 'closed'
    if ($foreign.rejected -ne $true -or $closed.rejected -ne $true) {
        throw 'The native hook accepted a foreign or closed SMB parent.'
    }
    Close-LabPipe $process
    Close-LabPipe $nativeProcess
    return [ordered]@{
        result = 'read_only_native_parent_hook_measured'
        opened = $opened
        independent_native = $native
        initial_metadata_unchanged = $true
        malformed_and_replayed = $malformed
        metadata_only = $metadataOnly
        unrelated_parent = $unrelated
        file_refusals = $files
        paused_parent_replacement = $parentResult
        fresh_replacement = $fresh
        paused_child_replacement = $childResult
        restored = $restored
        foreign_tree = $foreign
        closed_parent = $closed
        limits = 'Generated read-only Linux/ext4 Samba 4.17.12 hook only. Fixed fixture revision and role inventory, not production settings/bindings. No media streaming, writes, authenticated provider or production acceptance.'
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
        if (-not $CompilerImage) { throw "An already installed Linux x86_64 Rust $requiredVersion SDK is required. No base image was pulled." }
    }
    $base = ((Invoke-Docker -Arguments @('image', 'inspect', $CompilerImage)) -join "`n" | ConvertFrom-Json)[0]
    if ($base.Os -ne 'linux' -or $base.Architecture -ne 'amd64' -or -not @($base.Config.Env).Contains("RUST_VERSION=$requiredVersion")) {
        throw "This experiment requires an already installed Linux x86_64 Rust $requiredVersion SDK."
    }
    Invoke-Docker -Arguments @('volume', 'create', '--label', "io.domus.storage-smb=$run", $volume) | Out-Null
    $createdVolume = $true
    Invoke-Docker -Arguments @('network', 'create', '--internal', '--label', "io.domus.storage-smb=$run", $network) | Out-Null
    $createdNetwork = $true
    # Internet access is limited to the approved disposable package installation.
    $installCommand = 'apt-get update -qq && apt-get install -y --no-install-recommends samba smbclient cifs-utils && apt-get clean && mkdir -p /mounts/media /mounts/media-alias /mounts/staging /mounts/private-alias /fixture/tools/scripts /fixture/tools/crates/domus/src/infrastructure/filesystem /fixture/tools/tmp /fixture/payload/staging /fixture/private && chmod 755 /fixture /fixture/payload /fixture/payload/staging /fixture/private && printf "generated nested export control\n" > /fixture/payload/staging/sentinel && printf "generated private export control\n" > /fixture/private/sentinel'
    if ($AllowWitnessTools) {
        $installCommand += @'
 && curl --fail --silent --show-error --max-time 120 --proto '=https' https://files.pythonhosted.org/packages/c5/32/52ee34636aea1ab3f8baeeae1c3599461b8cddfb379862cc72ac61b8d91c/uv-0.13.8-py3-none-manylinux_2_17_x86_64.manylinux2014_x86_64.whl -o /tmp/uv.whl &&
 echo 'e18a57b8b352eb7c8feb317b9f6f533c8f0ad9673fd6cf34bbe3cf1a9f6a8984  /tmp/uv.whl' | sha256sum -c - &&
 mkdir -p /opt/domus-tools &&
 python3 -c 'import zipfile; data=zipfile.ZipFile("/tmp/uv.whl").read("uv-0.13.8.data/scripts/uv"); assert data[:4] == b"\x7fELF"; open("/opt/domus-tools/uv", "xb").write(data)' &&
 chmod 755 /opt/domus-tools/uv &&
 UV_PYTHON_DOWNLOADS=never /opt/domus-tools/uv venv --no-managed-python --python /usr/bin/python3 /opt/domus-smb &&
 UV_PYTHON_DOWNLOADS=never /opt/domus-tools/uv --no-cache pip install --python /opt/domus-smb/bin/python smbprotocol==1.17.0 &&
 rm /tmp/uv.whl
'@
    }
    if ($AllowServerHook) {
        $installCommand += ' && printf "generated private control\n" > /fixture/private/sentinel && mkdir /fixture/private/mounted && /bin/sh /tmp/storage-smb-hook-build.sh'
    }
    if ($AllowAuthenticatedWitness) {
        $installCommand += ' && test -z "$(getent passwd domus_a)" && test -z "$(getent passwd domus_b)" && useradd -u 10001 -M -s /usr/sbin/nologin domus_a && useradd -u 10002 -M -s /usr/sbin/nologin domus_b'
    }
    $installer = New-Container -Arguments @(
        '--network', 'bridge', '--env', 'DEBIAN_FRONTEND=noninteractive',
        '--env', "DOMUS_SMB_HOOK_BUILD_APPROVED=$([int][bool]$AllowServerHook)",
        '--mount', "type=volume,source=$volume,target=/fixture",
        '--entrypoint', '/bin/sh', $base.Id, '-c',
        $installCommand
    )
    if ($AllowServerHook) {
        foreach ($file in @('storage-smb-hook-build.sh', 'storage-smb-native-probe.c', 'storage-smb-auth-probe.h', 'storage-smb-auth-policy.rs')) {
            Invoke-Docker -Arguments @(
                'cp', (Join-Path $PSScriptRoot $file), "${installer}:/tmp/$file"
            ) | Out-Null
        }
        Invoke-Docker -Arguments @(
            'cp', (Join-Path $workspace 'crates\domus\src\infrastructure\filesystem\topology_prototype.rs'),
            "${installer}:/tmp/domus-topology-prototype.rs"
        ) | Out-Null
    }
    $installation = Start-Checked $installer
    $configFile = if ($AllowAuthenticatedWitness) { 'storage-smb-auth-lab.conf' } else { 'storage-smb-lab.conf' }
    Invoke-Docker -Arguments @('cp', (Join-Path $PSScriptRoot $configFile), "${installer}:/etc/samba/smb.conf") | Out-Null
    Invoke-Docker -Arguments @('cp', (Join-Path $PSScriptRoot 'storage-topology-prototype.rs'), "${installer}:/fixture/tools/scripts/storage-topology-prototype.rs") | Out-Null
    Invoke-Docker -Arguments @('cp', (Join-Path $workspace 'crates\domus\src\infrastructure\filesystem\topology_prototype.rs'), "${installer}:/fixture/tools/crates/domus/src/infrastructure/filesystem/topology_prototype.rs") | Out-Null
    if ($AllowWitnessTools) {
        Invoke-Docker -Arguments @('cp', (Join-Path $PSScriptRoot 'storage-smb-witness.py'), "${installer}:/fixture/tools/storage-smb-witness.py") | Out-Null
    }
    if ($AllowAuthenticatedWitness) {
        Invoke-Docker -Arguments @('cp', (Join-Path $PSScriptRoot 'storage-smb-auth-witness.py'), "${installer}:/fixture/tools/storage-smb-auth-witness.py") | Out-Null
    }
    $imageId = ((Invoke-Docker -Arguments @('commit', $installer, $imageTag)) -join "`n").Trim()
    if ($imageId -notmatch '^sha256:[a-f0-9]{64}$') { throw 'Unexpected generated image ID.' }
    $buildCommand = 'rustc --version && rustc --edition 2024 --deny warnings /fixture/tools/scripts/storage-topology-prototype.rs -o /fixture/tools/probe && smbd --version && smbclient --version'
    if ($AllowWitnessTools) {
        $buildCommand += ' && /opt/domus-tools/uv --version && /opt/domus-tools/uv --no-cache pip freeze --python /opt/domus-smb/bin/python && /opt/domus-smb/bin/python /fixture/tools/storage-smb-witness.py --self-test'
    }
    if ($AllowAuthenticatedWitness) {
        $buildCommand += ' && /opt/domus-smb/bin/python /fixture/tools/storage-smb-auth-witness.py --self-test'
    }
    if ($AllowReadOnlyOperations) {
        $buildCommand += ' && /opt/domus-smb/bin/python /fixture/tools/storage-smb-auth-witness.py --setup-read-files'
    }
    $builder = New-Container -Arguments @(
        '--network', 'none', '--read-only', '--cap-drop', 'ALL',
        '--security-opt', 'no-new-privileges', '--env', 'TMPDIR=/fixture/tools/tmp',
        '--env', "DOMUS_SMB_LAB_RUN=$run",
        '--mount', "type=volume,source=$volume,target=/fixture",
        '--entrypoint', '/bin/sh', $imageId, '-c',
        $buildCommand
    )
    $versions = Start-Checked $builder
    if ($versions -notmatch "^rustc $([regex]::Escape($requiredVersion)) ") {
        throw 'The actual compiler does not match the repository pin.'
    }
    if ($AllowWitnessTools -and $versions -notmatch '(?m)^Version 4\.17\.12(?:-|\s|$)') {
        throw 'The native witness experiment requires the reviewed Samba 4.17.12 implementation.'
    }
    if ($AllowAuthenticatedWitness) {
        Measure-AuthenticatedProof | ConvertTo-Json -Depth 12
        return
    }
    $server = New-Server
    $first = New-Client
    $second = New-Client
    Wait-Server $first $server
    $a = Read-Exports $first
    $b = Read-Exports $second
    $mounted = @()
    $recreatedMounted = $null
    if ($AllowCifsMount) {
        Mount-Exports $first
        Mount-Exports $second
        $mounted = @((Read-Mounted $first), (Read-Mounted $second))
        if ($mounted[0].roots[2].identity -eq $mounted[1].roots[2].identity) {
            throw 'The expected namespace-specific CIFS identity counterexample was not reproduced.'
        }
    }
    $serials = [ordered]@{}
    foreach ($export in @('media', 'media-alias', 'staging', 'private-alias')) {
        if ($a[$export] -notmatch '(?m)^Volume:.* serial number (0x[a-fA-F0-9]+)\s*$') {
            throw "SMB volume serial evidence is unavailable: $export"
        }
        $serials[$export] = $Matches[1].ToLowerInvariant()
    }
    if ($serials['media'] -eq $serials['media-alias']) {
        throw 'This server did not reproduce distinct volume serials for the same backing directory.'
    }
    $native = ((Invoke-Docker -Arguments @(
        'exec', $server, '/fixture/tools/probe', '/fixture', '/fixture/private',
        '/fixture/payload', '/fixture/payload/staging'
    )) -join "`n" | ConvertFrom-Json)
    $nested = @($native.relationships | Where-Object { $_.left -eq 2 -and $_.right -eq 3 })
    $sibling = @($native.relationships | Where-Object { $_.left -eq 1 -and $_.right -eq 2 })
    if ($nested.Count -ne 1 -or $nested[0].result -ne 'Overlapping' -or $sibling.Count -ne 1 -or $sibling[0].result -ne 'Separated') {
        throw 'The actual generated backing relationship was not established.'
    }
    $witness = $null
    if ($AllowWitnessTools) { $witness = Measure-Witness $server $first $second $native }
    $serverHook = $null
    if ($AllowServerHook) { $serverHook = Measure-ServerHook $server $first $second }
    $cifsParent = $null
    if ($AllowWitnessTools -and $AllowCifsMount) { $cifsParent = Measure-CifsParent $first }
    Invoke-Docker -Arguments @('stop', '--time', '5', $server) | Out-Null
    $offline = & docker exec $first smbclient '//storage-server/staging' '-N' '-m' 'SMB3' '-t' '2' '-c' 'ls' 2>&1
    if ($LASTEXITCODE -eq 0) { throw 'The offline server unexpectedly returned successful evidence.' }
    Invoke-Docker -Arguments @('rm', $server) | Out-Null
    $containers.Remove($server) | Out-Null
    $replacement = New-Server
    Wait-Server $first $replacement
    $reconnected = Read-Exports $first
    foreach ($export in @('media', 'media-alias', 'staging', 'private-alias')) {
        foreach ($response in @($b[$export], $reconnected[$export])) {
            if ($response -notmatch '(?m)^Volume:.* serial number (0x[a-fA-F0-9]+)\s*$' -or
                $Matches[1].ToLowerInvariant() -ne $serials[$export]) {
                throw 'Unchanged generated exports did not retain their reported serial values.'
            }
        }
    }
    $recreatedNative = ((Invoke-Docker -Arguments @(
        'exec', $replacement, '/fixture/tools/probe', '/fixture', '/fixture/private',
        '/fixture/payload', '/fixture/payload/staging'
    )) -join "`n" | ConvertFrom-Json)
    $recreatedWitness = $null
    if ($AllowWitnessTools) {
        $recreatedWitness = Measure-Witness $replacement $first $second $recreatedNative
        foreach ($role in @('media', 'media-alias', 'staging', 'private-alias')) {
            $originalRoot = $witness.native.roots.PSObject.Properties[$role].Value
            $newRoot = $recreatedWitness.native.roots.PSObject.Properties[$role].Value
            if ($originalRoot.native_identity -ne $newRoot.native_identity -or
                $originalRoot.filesystem_uuid -ne $newRoot.filesystem_uuid) {
                throw 'The native identity or filesystem UUID changed across generated server recreation.'
            }
            $recreatedServerHook = $null
            if ($AllowServerHook) { $recreatedServerHook = Measure-ServerHook $replacement $first $second }
        }
    }
    if ($AllowCifsMount) {
        $newClient = New-Client
        Wait-Server $newClient $replacement
        Mount-Exports $newClient
        $recreatedMounted = Read-Mounted $newClient
        if ($recreatedMounted.roots[2].identity -eq $mounted[0].roots[2].identity) {
            throw 'The expected changed CIFS identity after client recreation was not reproduced.'
        }
    }
    for ($index = 0; $index -lt 4; $index++) {
        if ($native.roots[$index].identity -ne $recreatedNative.roots[$index].identity) {
            throw 'Generated backing identities changed after server recreation.'
        }
    }
    $filesystems = (Invoke-Docker -Arguments @('exec', $first, 'cat', '/proc/filesystems')) -join "`n"
    $cifsPresent = $filesystems -match '(?m)^\s*(?:nodev\s+)?cifs\s*$'
    if ($AllowCifsMount -and -not $cifsPresent) {
        throw 'Mounted-client evidence was returned without registered CIFS support.'
    }
    $networkDetails = ((Invoke-Docker -Arguments @('network', 'inspect', $network)) -join "`n" | ConvertFrom-Json)[0]
    $hookMount = $null
    if ($AllowServerHook) {
        Invoke-Docker -Arguments @('rm', '--force', $replacement) | Out-Null
        $containers.Remove($replacement) | Out-Null
        $replacement = New-Server $true
        Wait-Server $first $replacement
        $mountProcess = Start-LabPipe $first '--server-hook'
        Read-LabPipe $mountProcess | Out-Null
        $hookMount = Send-LabPipe $mountProcess 'mount-case'
        if ($hookMount.result -ne 'native_refusal' -or $hookMount.errno -ne 18 -or
            $hookMount.child_native_identity -ne $native.roots[3].identity -or
            $null -ne $hookMount.marker) {
            throw 'The hook did not reject the actual native child mount crossing.'
        }
        Close-LabPipe $mountProcess
    }
    $serverDetails = ((Invoke-Docker -Arguments @('inspect', $replacement)) -join "`n" | ConvertFrom-Json)[0]
    if (-not $networkDetails.Internal -or (
        $serverDetails.HostConfig.PortBindings -and
        @($serverDetails.HostConfig.PortBindings.PSObject.Properties).Count -gt 0
    )) {
        throw 'The generated server was not isolated on an internal network without host ports.'
    }
    [ordered]@{
        result = 'smb_export_counterexample_measured'
        fixture_run = $run
        base_image = $base.Id
        versions = $versions
        installation_output = $installation
        source_hashes = [ordered]@{
            harness = (Get-FileHash $PSCommandPath -Algorithm SHA256).Hash
            config = (Get-FileHash (Join-Path $PSScriptRoot 'storage-smb-lab.conf') -Algorithm SHA256).Hash
            probe = (Get-FileHash (Join-Path $PSScriptRoot 'storage-topology-prototype.rs') -Algorithm SHA256).Hash
            policy = (Get-FileHash (Join-Path $workspace 'crates\domus\src\infrastructure\filesystem\topology_prototype.rs') -Algorithm SHA256).Hash
            witness = if ($AllowWitnessTools) { (Get-FileHash (Join-Path $PSScriptRoot 'storage-smb-witness.py') -Algorithm SHA256).Hash } else { $null }
            server_hook = if ($AllowServerHook) { (Get-FileHash (Join-Path $PSScriptRoot 'storage-smb-native-probe.c') -Algorithm SHA256).Hash } else { $null }
            hook_builder = if ($AllowServerHook) { (Get-FileHash (Join-Path $PSScriptRoot 'storage-smb-hook-build.sh') -Algorithm SHA256).Hash } else { $null }
        }
        backing = $native
        recreated_backing = $recreatedNative
        clients = @($a, $b)
        reported_volume_serials = $serials
        reconnected = $reconnected
        offline_error = $offline -join "`n"
        cifs_registered = $cifsPresent
        isolation = 'Verified internal Docker network and no published host ports.'
        mounted_clients = $mounted
        recreated_mounted_client = $recreatedMounted
        native_open_witness = $witness
        recreated_native_open_witness = $recreatedWitness
        cifs_parent_descriptor = $cifsParent
        server_parent_hook = $serverHook
        recreated_server_parent_hook = $recreatedServerHook
        server_hook_mount_refusal = $hookMount
        automatic_module_loading_approved = [bool] $AllowAutomaticCifsModuleLoad
        mounted_client_test = if ($AllowCifsMount) {
            'Actual retained CIFS handles exercised with read-only isolated mounts. Remote equal/nested/private aliases never receive Separated. This is rejection evidence, not remote binding admission or Windows SMB acceptance.'
        } else {
            'Not run. Protocol-only mode does not load host drivers or mount shares.'
        }
        evidence_scope = 'Actual generated Samba exports and known native backing relationships. Anonymous read-only SMB on internal Docker network only, no host ports. Equal/nested aliases and private-root export controls, two clients, offline failure and server recreation. Transport text/content are not separation authority. Runtime integration remains blocked.'
    } | ConvertTo-Json -Depth 12
}
finally {
    $cleanupErrors = @()
    foreach ($process in $pipes) {
        try {
            if (-not $process.HasExited) {
                $process.StandardInput.Close()
                if (-not $process.WaitForExit(5000)) {
                    $process.Kill($true)
                    $process.WaitForExit()
                }
            }
            $process.Dispose()
        }
        catch { $cleanupErrors += $_.Exception.Message }
    }
    foreach ($container in $containers) {
        try { Invoke-Docker -Arguments @('rm', '--force', $container) | Out-Null }
        catch { $cleanupErrors += $_.Exception.Message }
    }
    if ($createdNetwork) {
        try { Invoke-Docker -Arguments @('network', 'rm', $network) | Out-Null }
        catch { $cleanupErrors += $_.Exception.Message }
    }
    if ($createdVolume) {
        try { Invoke-Docker -Arguments @('volume', 'rm', $volume) | Out-Null }
        catch { $cleanupErrors += $_.Exception.Message }
    }
    foreach ($secretVolume in $secretVolumes) {
        try { Invoke-Docker -Arguments @('volume', 'rm', $secretVolume) | Out-Null }
        catch { $cleanupErrors += $_.Exception.Message }
    }
    if ($imageId) {
        try { Invoke-Docker -Arguments @('image', 'rm', $imageTag) | Out-Null }
        catch { $cleanupErrors += $_.Exception.Message }
    }
    if ($cleanupErrors.Count -gt 0) { throw "Generated SMB lab cleanup failed: $($cleanupErrors -join '; ')" }
}

[CmdletBinding()]
param(
    [ValidateSet('Both', 'Windows', 'Linux')]
    [string] $Platform = 'Both',
    [string] $CompilerImage = 'sha256:e6f2cff35d0f309acf8433b4f05878ed77092602c5065cc81801177da0bb3a9c',
    [ValidateSet('RuntimeHooks', 'PinnedSource')]
    [string] $Adapter = 'RuntimeHooks',
    [string] $EvidencePath,
    [switch] $RustOwnership,
    [switch] $IndependentOwners
)

$ErrorActionPreference = 'Stop'
if ($RustOwnership -and $Adapter -ne 'PinnedSource') {
    throw 'The Rust ownership proof requires PinnedSource explicitly.'
}
if ($IndependentOwners -and (-not $RustOwnership -or $Adapter -ne 'PinnedSource')) {
    throw 'Independent owners require both RustOwnership and PinnedSource explicitly.'
}
$repository = Split-Path -Parent $PSScriptRoot
$run = [Guid]::NewGuid().ToString('N')
$scratch = Join-Path ([IO.Path]::GetTempPath()) "domus-sqlite-$run"
$container = "domus-sqlite-$run"
$volume = "domus-sqlite-$run"
$createdContainer = $false
$createdVolume = $false
$junction = $null
$result = [ordered]@{ run = $run; sqlite = '3.53.2'; adapter = $Adapter; production_adapter = $false; independent_owners = [bool]$IndependentOwners }

function Invoke-Checked {
    param([string] $Program, [string[]] $Arguments)
    $output = & $Program @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$Program failed with exit $LASTEXITCODE.`n$($output -join "`n")"
    }
    return $output
}

function Invoke-BoundedOwnership {
    param([string] $Program)
    $start = [Diagnostics.ProcessStartInfo]::new($Program)
    $start.UseShellExecute = $false
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $start.ArgumentList.Add('--test-threads=1')
    $start.ArgumentList.Add('--nocapture')
    $process = [Diagnostics.Process]::Start($start)
    try {
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit(120000)) {
            Stop-Process -Id $process.Id -Force
            $process.WaitForExit()
            throw 'The independent-owner fixture exceeded its 120-second limit.'
        }
        $output = $stdout.GetAwaiter().GetResult()
        $errors = $stderr.GetAwaiter().GetResult()
        if ($process.ExitCode -ne 0) {
            throw "Independent-owner fixture failed with exit $($process.ExitCode).`n$output`n$errors"
        }
        if ($errors) { [Console]::Error.WriteLine($errors.TrimEnd()) }
        return $output -split '\r?\n'
    }
    finally { $process.Dispose() }
}

function ConvertFrom-OwnershipProof {
    param([string[]] $OutputLines)
    $marker = if ($IndependentOwners) { 'DOMUS_SQLITE_INDEPENDENT_EVIDENCE=' } else { 'DOMUS_SQLITE_OWNERSHIP_EVIDENCE=' }
    $summary = @($OutputLines | Where-Object { $_.Contains($marker) })
    if ($summary.Count -ne 1) { throw 'Expected one completed Rust ownership proof.' }
    return $summary[0].Substring($summary[0].IndexOf($marker) + $marker.Length) | ConvertFrom-Json
}

try {
    New-Item -ItemType Directory -Path $scratch | Out-Null
    $lock = Get-Content -LiteralPath (Join-Path $repository 'Cargo.lock') -Raw
    $package = [regex]::Match($lock, '(?ms)^\[\[package\]\]\r?\nname = "libsqlite3-sys"\r?\nversion = "([^"]+)".*?^checksum = "([a-f0-9]+)"')
    if (-not $package.Success) { throw 'Pinned SQLite package checksum unavailable.' }
    $version = $package.Groups[1].Value
    $cache = Join-Path $env:USERPROFILE '.cargo\registry\cache'
    $archives = @(Get-ChildItem -LiteralPath $cache -Directory |
        ForEach-Object { Join-Path $_.FullName "libsqlite3-sys-$version.crate" } |
        Where-Object { Test-Path -LiteralPath $_ })
    if ($archives.Count -ne 1) { throw 'One installed pinned SQLite archive is required. No packages are installed by this script.' }
    $archiveHash = (Get-FileHash -LiteralPath $archives[0] -Algorithm SHA256).Hash
    if ($archiveHash -ne $package.Groups[2].Value) { throw 'SQLite archive does not match Cargo.lock.' }
    Invoke-Checked tar @('-xf', $archives[0], '-C', $scratch, '--strip-components=2',
        "libsqlite3-sys-$version/sqlite3/sqlite3.c", "libsqlite3-sys-$version/sqlite3/sqlite3.h") | Out-Null
    $result.archive_sha256 = $archiveHash
    $result.engine_sha256 = (Get-FileHash -LiteralPath (Join-Path $scratch 'sqlite3.c')).Hash
    $result.header_sha256 = (Get-FileHash -LiteralPath (Join-Path $scratch 'sqlite3.h')).Hash
    $tempHeader = Join-Path $PSScriptRoot 'storage-sqlite-temp-workload.h'
    Copy-Item -LiteralPath $tempHeader -Destination $scratch
    $result.temp_header_sha256 = (Get-FileHash -LiteralPath $tempHeader).Hash
    $definitions = @('SQLITE_CORE', 'SQLITE_DEFAULT_FOREIGN_KEYS=1',
        'SQLITE_ENABLE_API_ARMOR', 'SQLITE_ENABLE_COLUMN_METADATA', 'SQLITE_ENABLE_DBSTAT_VTAB',
        'SQLITE_ENABLE_FTS3', 'SQLITE_ENABLE_FTS3_PARENTHESIS', 'SQLITE_ENABLE_FTS5',
        'SQLITE_ENABLE_JSON1', 'SQLITE_ENABLE_LOAD_EXTENSION=1', 'SQLITE_ENABLE_MEMORY_MANAGEMENT',
        'SQLITE_ENABLE_RTREE', 'SQLITE_ENABLE_STAT4', 'SQLITE_SOUNDEX', 'SQLITE_THREADSAFE=1',
        'SQLITE_USE_URI')
    $probeFlags = ''
    $windowsProbeFlags = ''
    if ($Adapter -eq 'PinnedSource') {
        $patch = Join-Path $PSScriptRoot 'storage-sqlite-source-adapter.patch'
        $nativeHeader = Join-Path $PSScriptRoot 'storage-sqlite-native-io.h'
        $result.patch_sha256 = (Get-FileHash -LiteralPath $patch).Hash
        $result.native_header_sha256 = (Get-FileHash -LiteralPath $nativeHeader).Hash
        Copy-Item -LiteralPath $nativeHeader -Destination $scratch
        Invoke-Checked git @('-C', $scratch, 'apply', '--check', $patch) | Out-Null
        Invoke-Checked git @('-C', $scratch, 'apply', $patch) | Out-Null
        $result.patched_engine_sha256 = (Get-FileHash -LiteralPath (Join-Path $scratch 'sqlite3.c')).Hash
        $definitions += 'DOMUS_SQLITE_SOURCE_ADAPTER'
        $probeFlags = '-DDOMUS_SQLITE_SOURCE_ADAPTER'
        $windowsProbeFlags = '/DDOMUS_SQLITE_SOURCE_ADAPTER'
    }

    if ($RustOwnership) {
        $pin = Select-String -LiteralPath (Join-Path $repository 'rust-toolchain.toml') -Pattern '^channel\s*=\s*"(\d+\.\d+\.\d+)"'
        if (@($pin).Count -ne 1) { throw 'An exact Rust pin is required.' }
        $requiredVersion = $pin.Matches[0].Groups[1].Value
        $rust = Join-Path $PSScriptRoot 'storage-sqlite-root-ownership.rs'
        if ($IndependentOwners) {
            $rust = Join-Path $PSScriptRoot 'storage-sqlite-independent-owners.rs'
            $definitions += 'DOMUS_SQLITE_CONCURRENT'
            $probeFlags += ' -DDOMUS_SQLITE_CONCURRENT'
            $windowsProbeFlags += ' /DDOMUS_SQLITE_CONCURRENT'
            foreach ($header in @('storage-sqlite-probe-lock.h', 'storage-sqlite-independent-native.h')) {
                Copy-Item -LiteralPath (Join-Path $PSScriptRoot $header) -Destination $scratch
                $result[$header] = (Get-FileHash -LiteralPath (Join-Path $scratch $header)).Hash
            }
        }
        $result.ownership_rust_sha256 = (Get-FileHash -LiteralPath $rust).Hash
    }

    if ($Platform -in @('Both', 'Linux')) {
        $image = Invoke-Checked docker @('image', 'inspect', $CompilerImage) | ConvertFrom-Json
        $result.linux_image = $image.Id
        Invoke-Checked docker @('volume', 'create', '--label', "domus.sqlite.run=$run", $volume) | Out-Null
        $createdVolume = $true
        Invoke-Checked docker @('run', '-d', '--pull=never', '--name', $container,
            '--label', "domus.sqlite.run=$run", '--network', 'none', '--read-only',
            '--cap-drop', 'ALL', '--memory', '512m', '--memory-swap', '512m',
            '--cpus', '1', '--pids-limit', '128',
            '--mount', "type=volume,source=$volume,target=/fixture",
            '--tmpfs', '/tmp:rw,nosuid,nodev,size=32m', $CompilerImage, 'sleep', '600') | Out-Null
        $createdContainer = $true
        $inspection = Invoke-Checked docker @('inspect', $container) | ConvertFrom-Json
        $hostConfig = $inspection.HostConfig
        if (-not $inspection.State.Running -or $hostConfig.NetworkMode -ne 'none' -or
            $hostConfig.Privileged -or -not $hostConfig.ReadonlyRootfs -or
            $hostConfig.CapAdd.Count -or $hostConfig.CapDrop -notcontains 'ALL' -or
            $hostConfig.Memory -ne 536870912 -or $hostConfig.MemorySwap -ne 536870912 -or
            $hostConfig.NanoCpus -ne 1000000000 -or $hostConfig.PidsLimit -ne 128 -or
            $inspection.Mounts.Count -ne 1 -or $inspection.Mounts[0].Type -ne 'volume' -or
            $inspection.Mounts[0].Name -ne $volume -or $inspection.Mounts[0].Destination -ne '/fixture' -or
            $hostConfig.Tmpfs.'/tmp' -ne 'rw,nosuid,nodev,size=32m' -or
            @($hostConfig.PortBindings.PSObject.Properties).Count) {
            throw 'Generated Linux fixture isolation differs from the approved scope.'
        }
        $result.linux_compiler = Invoke-Checked docker @('exec', $container, 'gcc', '--version')
        Invoke-Checked docker @('exec', $container, 'mkdir', '-m', '700', '/fixture/src') | Out-Null
        Invoke-Checked docker @('exec', $container, 'mkdir', '-m', '1777', '/fixture/run') | Out-Null
        foreach ($file in @('sqlite3.c', 'sqlite3.h', 'storage-sqlite-temp-workload.h')) {
            Invoke-Checked docker @('cp', (Join-Path $scratch $file), "${container}:/fixture/src/$file") | Out-Null
        }
        if ($Adapter -eq 'PinnedSource') {
            Invoke-Checked docker @('cp', (Join-Path $scratch 'storage-sqlite-native-io.h'),
                "${container}:/fixture/src/storage-sqlite-native-io.h") | Out-Null
        }
        if ($IndependentOwners) {
            foreach ($header in @('storage-sqlite-probe-lock.h', 'storage-sqlite-independent-native.h')) {
                Invoke-Checked docker @('cp', (Join-Path $scratch $header), "${container}:/fixture/src/$header") | Out-Null
            }
        }
        $source = Join-Path $PSScriptRoot 'storage-sqlite-root-prototype.c'
        $result.linux_probe_sha256 = (Get-FileHash -LiteralPath $source).Hash
        Invoke-Checked docker @('cp', $source, "${container}:/fixture/src/probe.c") | Out-Null
        $flags = ($definitions | ForEach-Object { "-D$_" }) -join ' '
        if ($RustOwnership) {
            $rustVersion = Invoke-Checked docker @('exec', $container, 'rustc', '--version')
            if ($rustVersion -notmatch "^rustc $([regex]::Escape($requiredVersion)) ") {
                throw 'The installed Linux Rust compiler does not match the repository pin.'
            }
            $result.linux_rust_version = $rustVersion
            $native = Join-Path $PSScriptRoot 'storage-sqlite-ownership-linux.c'
            $result.linux_ownership_native_sha256 = (Get-FileHash -LiteralPath $native).Hash
            Invoke-Checked docker @('cp', $native, "${container}:/fixture/src/ownership-native.c") | Out-Null
            Invoke-Checked docker @('cp', $rust, "${container}:/fixture/src/ownership.rs") | Out-Null
            $command = "set -eu; cd /fixture/src; gcc -O0 -fPIC $flags -DHAVE_USLEEP=1 -DHAVE_ISNAN -D_POSIX_THREAD_SAFE_FUNCTIONS -c sqlite3.c -o sqlite3.o; gcc -O0 -fPIC -Wall -Wextra -Werror $probeFlags -c ownership-native.c -o ownership-native.o; ar rcs libdomus_ownership.a sqlite3.o ownership-native.o; rustc --edition 2024 --deny warnings --test ownership.rs -L native=. -l static=domus_ownership -l dl -l pthread -l m -o /fixture/ownership"
        }
        else {
            $command = "set -eu; cd /fixture/src; gcc -O0 $flags -DHAVE_USLEEP=1 -DHAVE_ISNAN -D_POSIX_THREAD_SAFE_FUNCTIONS -c sqlite3.c -o sqlite3.o; gcc -O0 -Wall -Wextra -Werror $probeFlags probe.c sqlite3.o -o /fixture/probe -ldl -lpthread -lm"
        }
        Invoke-Checked docker @('exec', $container, 'sh', '-c', $command) | Out-Null
        $result.linux_uid = Invoke-Checked docker @('exec', '--user', '1000:1000', $container, 'id', '-u')
        if ($result.linux_uid -ne '1000') { throw 'Ordinary fixture account check failed.' }
        if ($RustOwnership) {
            Invoke-Checked docker @('exec', '--user', '1000:1000', $container,
                'mkdir', '-m', '700', '/fixture/run/ownership') | Out-Null
            $output = Invoke-Checked docker @('exec', '--user', '1000:1000',
                '-e', 'DOMUS_SQLITE_OWNERSHIP_ROOT=/fixture/run/ownership', $container,
                'timeout', '120s', '/fixture/ownership', '--test-threads=1', '--nocapture')
            $result.linux_rust_ownership = ConvertFrom-OwnershipProof $output
        }
        else {
            $result.linux = Invoke-Checked docker @('exec', '--user', '1000:1000', $container,
                'timeout', '120s', '/fixture/probe', '/fixture/run/cases') | ConvertFrom-Json
        }
    }

    if ($Platform -in @('Both', 'Windows')) {
        $discovery = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
        $installations = Invoke-Checked $discovery @('-all', '-products', '*', '-format', 'json') | ConvertFrom-Json
        $vcvars = $installations | ForEach-Object {
            Join-Path $_.installationPath 'VC\Auxiliary\Build\vcvars64.bat'
        } | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
        if (-not $vcvars) { throw 'Installed complete MSVC environment unavailable.' }
        $source = Join-Path $PSScriptRoot 'storage-sqlite-windows-prototype.c'
        $result.windows_probe_sha256 = (Get-FileHash -LiteralPath $source).Hash
        Copy-Item -LiteralPath $source -Destination (Join-Path $scratch 'probe.c')
        $flags = ($definitions | ForEach-Object { "/D$_" }) -join ' '
        if ($RustOwnership) {
            $rustVersion = Invoke-Checked rustup @('run', $requiredVersion, 'rustc', '--version')
            if ($rustVersion -notmatch "^rustc $([regex]::Escape($requiredVersion)) ") {
                throw 'The installed Rust toolchain does not match the repository pin.'
            }
            $native = Join-Path $PSScriptRoot 'storage-sqlite-ownership-windows.c'
            Copy-Item -LiteralPath $native -Destination (Join-Path $scratch 'ownership-native.c')
            Copy-Item -LiteralPath $rust -Destination (Join-Path $scratch 'ownership.rs')
            $result.rust_version = $rustVersion
            $result.ownership_native_sha256 = (Get-FileHash -LiteralPath $native).Hash
            $result.ownership_rust_sha256 = (Get-FileHash -LiteralPath $rust).Hash
            $build = "call `"$vcvars`" >nul && cd /d `"$scratch`" && cl /nologo /W0 /Od /MD $flags /D_CRT_SECURE_NO_WARNINGS /c sqlite3.c && cl /nologo /W4 /WX /Od /MD $windowsProbeFlags /D_CRT_SECURE_NO_WARNINGS /c ownership-native.c && lib /nologo /OUT:domus_ownership.lib sqlite3.obj ownership-native.obj && rustup run $requiredVersion rustc --edition 2024 --deny warnings --test ownership.rs -L native=. -l static=domus_ownership -l ntdll -o ownership.exe"
            $result.windows_build = Invoke-Checked $env:ComSpec @('/d', '/c', $build)
            $fixture = Join-Path $scratch 'ownership'
            New-Item -ItemType Directory -Path $fixture | Out-Null
            $previous = $env:DOMUS_SQLITE_OWNERSHIP_ROOT
            try {
                $env:DOMUS_SQLITE_OWNERSHIP_ROOT = $fixture
                if ($IndependentOwners) {
                    $output = Invoke-BoundedOwnership (Join-Path $scratch 'ownership.exe')
                }
                else {
                    $output = Invoke-Checked (Join-Path $scratch 'ownership.exe') @('--test-threads=1', '--nocapture')
                }
            }
            finally { $env:DOMUS_SQLITE_OWNERSHIP_ROOT = $previous }
            $result.rust_ownership = ConvertFrom-OwnershipProof $output
        }
        else {
            $build = "call `"$vcvars`" >nul && cd /d `"$scratch`" && cl /nologo /W0 /Od $flags /D_CRT_SECURE_NO_WARNINGS /c sqlite3.c && cl /nologo /W4 /WX /Od $windowsProbeFlags /D_CRT_SECURE_NO_WARNINGS probe.c sqlite3.obj ntdll.lib /Fe:probe.exe"
            $result.windows_build = Invoke-Checked $env:ComSpec @('/d', '/c', $build)
            $exe = Join-Path $scratch 'probe.exe'
            $case = Join-Path $scratch 'state'
            $result.windows = Invoke-Checked $exe @($case) | ConvertFrom-Json
            & $exe $case interrupt 0
            if ($LASTEXITCODE -ne 23) { throw 'Interrupted writer did not reach its expected exit.' }
            if (@(Get-ChildItem -LiteralPath (Join-Path $case 'sqlite-tmp') -Force).Count) {
                throw 'Interrupted temporary file cleanup failed.'
            }
            $result.windows_temp_interrupted_cleanup = $true
            $wal = Join-Path $case 'state.sqlite-wal'
            if (-not (Test-Path -LiteralPath $wal) -or (Get-Item -LiteralPath $wal).Length -eq 0) {
                throw 'Interrupted WAL evidence missing.'
            }
            Invoke-Checked $exe @($case, 'recover', '0') | Out-Null
            if (Test-Path -LiteralPath $wal) { throw 'Recovery WAL cleanup failed.' }
            $sentinel = Join-Path $scratch 'sentinel'
            New-Item -ItemType Directory -Path $sentinel | Out-Null
            $marker = Join-Path $sentinel 'unchanged'
            [IO.File]::WriteAllBytes($marker, [Text.Encoding]::ASCII.GetBytes('unchanged'))
            $before = (Get-FileHash -LiteralPath $marker).Hash
            $junction = $wal
            New-Item -ItemType Junction -Path $junction -Target $sentinel | Out-Null
            Invoke-Checked $exe @($case, 'reject-wal', '0') | Out-Null
            if ((Get-FileHash -LiteralPath $marker).Hash -ne $before -or
                -not ((Get-Item -LiteralPath $junction).Attributes -band [IO.FileAttributes]::ReparsePoint)) {
                throw 'Refused junction or target changed.'
            }
            $result.windows_recovery = @{ committed_rows = 103; uncommitted_value_absent = 999 }
            $result.windows_reparse = @{ target_unchanged = $true; junction_preserved = $true }
            [IO.Directory]::Delete($junction)
            $junction = $null
            $temporary = Join-Path $case 'sqlite-tmp'
            $heldTemporary = Join-Path $case 'sqlite-tmp-held'
            Move-Item -LiteralPath $temporary -Destination $heldTemporary
            $junction = $temporary
            New-Item -ItemType Junction -Path $junction -Target $sentinel | Out-Null
            Invoke-Checked $exe @($case, 'reject-temp', '0') | Out-Null
            if ((Get-FileHash -LiteralPath $marker).Hash -ne $before -or
                -not ((Get-Item -LiteralPath $junction).Attributes -band [IO.FileAttributes]::ReparsePoint) -or
                @(Get-ChildItem -LiteralPath $sentinel).Count -ne 1) {
                throw 'Refused temporary root junction or target changed.'
            }
            [IO.Directory]::Delete($junction)
            $junction = $null
            Move-Item -LiteralPath $heldTemporary -Destination $temporary
            $result.windows_temp_reparse = @{ target_unchanged = $true; junction_preserved = $true }
        }
    }
} finally {
    if ($createdContainer) { Invoke-Checked docker @('rm', '-f', $container) | Out-Null }
    if ($createdVolume) { Invoke-Checked docker @('volume', 'rm', $volume) | Out-Null }
    if ($junction -and (Test-Path -LiteralPath $junction)) {
        [IO.Directory]::Delete($junction)
    }
    if (Test-Path -LiteralPath $scratch) { Remove-Item -LiteralPath $scratch -Recurse -Force }
}

$result.harness_sha256 = (Get-FileHash -LiteralPath $PSCommandPath).Hash
$result.resources_removed = -not (Test-Path -LiteralPath $scratch)
$json = $result | ConvertTo-Json -Depth 8
if ($EvidencePath) { [IO.File]::WriteAllText($EvidencePath, $json + "`n", [Text.UTF8Encoding]::new($false)) }
$json

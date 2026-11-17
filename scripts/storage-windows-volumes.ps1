[CmdletBinding()]
param(
    [Parameter(Mandatory)][string] $SessionFiles,
    [Parameter(Mandatory)][string] $Run,
    [switch] $InterruptAfterFirstAttach,
    [switch] $SavedBindingProof
)
$ErrorActionPreference = 'Stop'
if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) {
    throw 'This generated-volume experiment requires Windows.'
}
$principal = [Security.Principal.WindowsPrincipal]::new(
    [Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'The approved generated disk operations require elevation.'
}
if ($Run -notmatch '^[a-f0-9]{32}$') { throw 'A generated run identifier is required.' }
$parent = [IO.Path]::GetFullPath($SessionFiles)
if (-not [IO.Directory]::Exists($parent)) { throw 'The selected session files directory must exist.' }
$fixture = Join-Path $parent "storage-vhd-$Run"
if (Test-Path -LiteralPath $fixture) { throw 'The generated fixture must not already exist.' }
$resultPath = Join-Path $parent "storage-vhd-$Run.json"
if (Test-Path -LiteralPath $resultPath) { throw 'The evidence file must be new.' }
Add-Type -LiteralPath @(
    (Join-Path $PSScriptRoot 'storage-windows-prototype.cs'),
    (Join-Path $PSScriptRoot 'storage-windows-volumes.cs')
)
$parentRoot = [StorageWindowsPrototype]::Open($parent)
$images = [Collections.Generic.List[StorageWindowsVolumes+Image]]::new()
$roots = [Collections.Generic.List[StorageWindowsPrototype+Root]]::new()
$assertions = [Collections.Generic.List[string]]::new()
$observations = [Collections.Generic.List[object]]::new()
$ownership = [Collections.Generic.List[object]]::new()
$cleanupErrors = [Collections.Generic.List[string]]::new()
$failure = $null
$interrupted = $false
$hostObservation = $null
$bytes = 0
$savedBinding = $null
$reattached = $null
$copyObservation = $null
$copiedPath = $null
$configPath = $null
$configCreated = $false

function Assert-True([bool] $Condition, [string] $Label) {
    if (-not $Condition) { throw $Label }
    $assertions.Add($Label)
}
function Owned-Disk($Image, [string] $ExpectedUniqueId) {
    $number = $Image.DiskNumber()
    $disk = Get-Disk -Number $number
    if ($disk.IsBoot -or $disk.IsSystem -or $disk.Size -ne 256MB -or $disk.BusType -ne 'File Backed Virtual') {
        throw 'The selected disk is not the bounded generated VHDX.'
    }
    if ($ExpectedUniqueId -and $disk.UniqueId -ne $ExpectedUniqueId) {
        throw 'The generated disk identity changed.'
    }
    return $disk
}
try {
    $parentRoot.Recheck()
    [IO.Directory]::CreateDirectory($fixture) | Out-Null
    foreach ($name in @('first', 'second')) {
        $path = Join-Path $fixture "$name.vhdx"
        $image = [StorageWindowsVolumes]::Create($path)
        $images.Add($image)
        $ownership.Add([ordered]@{
            path = $image.Path
            native_file_identity = $image.FileIdentity
            virtual_disk_id = $image.Id.ToString()
            initial_namespace_identifier = $image.InitialNamespaceIdentifier.ToString()
            capacity = [StorageWindowsVolumes+Image]::Capacity
        })
        [ordered]@{ run = $Run; generated_images = @($ownership.ToArray()) } |
            ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $resultPath -Encoding UTF8
        $image.Attach()
        if ($InterruptAfterFirstAttach) {
            $interrupted = $true
            break
        }
        if ($name -eq 'first') {
            try { throw [InvalidOperationException]::new('generated attach interruption') }
            catch [InvalidOperationException] {
                if ($_.Exception.Message -ne 'generated attach interruption') { throw }
            }
            finally { $image.Detach() }
            Assert-True (-not $image.Attached) 'controlled interruption detaches the first image'
            $image.Attach()
            Assert-True $image.Attached 'same first image reattaches without permanent attachment'
        }
        $disk = Owned-Disk $image ''
        $unique = $disk.UniqueId
        if ($disk.PartitionStyle -ne 'RAW') { throw 'The new disk is not uninitialized.' }
        Owned-Disk $image $unique | Initialize-Disk -PartitionStyle GPT | Out-Null
        $disk = Owned-Disk $image $unique
        $ownership[$ownership.Count - 1].initialized_namespace_identifier = $image.NamespaceIdentifier().ToString()
        $partition = $disk | New-Partition -UseMaximumSize
        $number = $partition.PartitionNumber
        $ownership[$ownership.Count - 1].partition_number = $number
        $disk = Owned-Disk $image $unique
        $partition = Get-Partition -DiskNumber $disk.Number -PartitionNumber $number
        if ($partition.DriveLetter) { throw 'An unexpected drive letter was assigned.' }
        $volume = $partition | Get-Volume
        $volume | Format-Volume -FileSystem NTFS -NewFileSystemLabel "Domus-$name" -Confirm:$false | Out-Null
        $disk = Owned-Disk $image $unique
        $partition = Get-Partition -DiskNumber $disk.Number -PartitionNumber $number
        $volume = $partition | Get-Volume
        if ($partition.DriveLetter -or $volume.FileSystemType -ne 'NTFS' -or
            $volume.Size -gt 256MB -or $volume.Path -notmatch '^\\\\\?\\Volume\{[a-fA-F0-9-]{36}\}\\$') {
            throw 'Unexpected formatted volume or drive letter.'
        }
        $rootPath = $volume.Path + 'fixture'
        [IO.Directory]::CreateDirectory($rootPath) | Out-Null
        [IO.Directory]::CreateDirectory($rootPath + '\child') | Out-Null
        $root = [StorageWindowsPrototype]::Open($rootPath)
        $roots.Add($root)
        $observations.Add([StorageWindowsVolumes]::Inspect($root, $image))
        Assert-True ($root.Child('child').Filesystem -eq 'NTFS') "$name parent-relative child"
        $alias = [StorageWindowsPrototype]::Open($root.Evidence.Coordinate)
        $roots.Add($alias)
        Assert-True ([StorageWindowsPrototype]::Relationship($root.Evidence, $alias.Evidence) -eq 'Overlapping') "$name volume alias overlaps"
    }
    if (-not $interrupted) {
        Assert-True ($observations.Count -eq 2) 'two actual volume observations'
        Assert-True ($observations[0].Disk -ne $observations[1].Disk) 'different generated disk extents'
        Assert-True ($observations[0].BackingIdentity -ne $observations[1].BackingIdentity) 'different retained backing images'
        Assert-True ($observations[0].Coordinate.Substring(0, 48) -ne $observations[1].Coordinate.Substring(0, 48)) 'different actual volume coordinates'
        Assert-True ([StorageWindowsPrototype]::Relationship($roots[0].Evidence, $roots[2].Evidence) -eq 'UnknownVolume') 'old policy refuses actual different volumes'
        Assert-True ([StorageWindowsPrototype]::Relationship($roots[2].Evidence, $roots[0].Evidence) -eq 'UnknownVolume') 'old policy refuses reversed different volumes'
        $wrongImageRefused = $false
        try { [StorageWindowsVolumes]::Inspect($roots[0], $images[1]) | Out-Null }
        catch {
            if ($_.Exception.ToString() -notmatch 'Volume extent is outside the exact generated disk') { throw }
            $wrongImageRefused = $true
        }
        Assert-True $wrongImageRefused 'actual wrong backing image is refused'
        $hostRoot = [StorageWindowsPrototype]::Open($fixture)
        $roots.Add($hostRoot)
        $hostObservation = $hostRoot.ReadRetained()
        foreach ($index in @(0, 1)) {
            $backing = $images[$index].BackingCoordinate()
            Assert-True ($backing.StartsWith($hostRoot.Evidence.Coordinate + '\', [StringComparison]::OrdinalIgnoreCase)) "image $index is actually backed inside the host private candidate"
            Assert-True ([string]::Equals($backing, $observations[$index].BackingPath, [StringComparison]::OrdinalIgnoreCase)) "image $index retained file matches its dependency coordinate"
            Assert-True ([StorageWindowsPrototype]::Relationship($hostRoot.Evidence, $roots[$index * 2].Evidence) -eq 'UnknownVolume') "image $index different volume cannot bypass host containment"
        }
        $hostRoot.Recheck()
        $bytes = ($images | ForEach-Object { (Get-Item -LiteralPath $_.Path).Length } | Measure-Object -Sum).Sum
        Assert-True ($bytes -le 512MB) 'backing files stay within the total limit'
        if ($SavedBindingProof) {
            $configPath = Join-Path $fixture 'config.json'
            $savedBinding = [ordered]@{
                format = 'generated_vhdx_binding_diagnostic_v1'
                state_binding = [ordered]@{
                    root_identity = $observations[0].RootIdentity
                    backing_file_identity = $images[0].FileIdentity
                    virtual_disk_id = $images[0].Id.ToString()
                }
            }
            $json = $savedBinding | ConvertTo-Json -Depth 4
            $config = [IO.File]::Open($configPath, [IO.FileMode]::CreateNew,
                [IO.FileAccess]::Write, [IO.FileShare]::Read)
            $configCreated = $true
            try {
                $encoded = [Text.UTF8Encoding]::new($false).GetBytes($json + "`n")
                $config.Write($encoded, 0, $encoded.Length)
                $config.Flush($true)
            } finally { $config.Dispose() }
            $configHash = (Get-FileHash -LiteralPath $configPath).Hash
            $saved = Get-Content -LiteralPath $configPath -Raw | ConvertFrom-Json
            $images[0].VerifySavedBinding($saved.state_binding.backing_file_identity,
                [Guid]$saved.state_binding.virtual_disk_id)
            Assert-True $true 'original saved backing pin matches actual retained image'
            foreach ($root in $roots) { $root.Dispose() }
            $roots.Clear()
            foreach ($image in $images) {
                $image.Detach()
                Assert-True (-not $image.Attached) 'all native root handles close before exact detach'
            }
            $images[0].VerifySavedBinding($saved.state_binding.backing_file_identity,
                [Guid]$saved.state_binding.virtual_disk_id)
            $images[0].Attach()
            $disk = Owned-Disk $images[0] ''
            $partition = Get-Partition -DiskNumber $disk.Number -PartitionNumber $ownership[0].partition_number
            $volume = $partition | Get-Volume
            Assert-True (-not $partition.DriveLetter -and $volume.FileSystemType -eq 'NTFS') 'reattached image stays NTFS without a drive letter'
            $root = [StorageWindowsPrototype]::Open($volume.Path + 'fixture')
            $roots.Add($root)
            $reattached = [StorageWindowsVolumes]::Inspect($root, $images[0])
            Assert-True ($reattached.RootIdentity -eq $saved.state_binding.root_identity) 'saved full native root identity survives detach and reattach'
            Assert-True ($reattached.BackingIdentity -eq $saved.state_binding.backing_file_identity) 'reattached root maps to the exact saved backing file'
            $images[0].VerifySavedBinding($saved.state_binding.backing_file_identity,
                [Guid]$saved.state_binding.virtual_disk_id)
            Assert-True ($root.Child('child').Filesystem -eq 'NTFS') 'fresh reattached authority performs parent-relative child work'
            foreach ($root in $roots) { $root.Dispose() }
            $roots.Clear()
            $images[0].Detach()

            $wrongPinRefused = $false
            try {
                $images[1].VerifySavedBinding($saved.state_binding.backing_file_identity,
                    [Guid]$saved.state_binding.virtual_disk_id)
            } catch {
                if ($_.Exception.ToString() -notmatch 'Saved backing binding does not match') { throw }
                $wrongPinRefused = $true
            }
            Assert-True $wrongPinRefused 'a different actual image cannot use the saved first-image pin'
            $second = $images[1]
            $copiedPath = $second.Path
            $second.Dispose()
            $images.RemoveAt(1)
            [IO.File]::Delete($copiedPath)
            Assert-True (-not [IO.File]::Exists($copiedPath)) 'remove original second image before using its bounded copy slot'
            $sourceHash = (Get-FileHash -LiteralPath $images[0].Path).Hash
            [IO.File]::Copy($images[0].Path, $copiedPath, $false)
            $copyObservation = [StorageWindowsVolumes]::InspectDetachedCopy($copiedPath)
            Assert-True ((Get-FileHash -LiteralPath $copiedPath).Hash -eq $sourceHash) 'detached copy has exactly the original image bytes'
            Assert-True ($copyObservation.VirtualDiskId -eq $images[0].Id) 'copied image preserves the virtual disk identifier'
            Assert-True ($copyObservation.NamespaceIdentifier -eq $images[0].NamespaceIdentifier()) 'copied image preserves the disk namespace identifier'
            Assert-True ($copyObservation.FileIdentity -ne $saved.state_binding.backing_file_identity) 'copied image has a different full native backing-file identity'
            $copiedPinRefused = $false
            try {
                [StorageWindowsVolumes]::VerifyDetachedCopyBinding($copyObservation,
                    $saved.state_binding.backing_file_identity, [Guid]$saved.state_binding.virtual_disk_id)
            } catch {
                if ($_.Exception.ToString() -notmatch 'Saved backing binding does not match') { throw }
                $copiedPinRefused = $true
            }
            Assert-True ($copiedPinRefused -and -not $copyObservation.Loaded) 'byte-identical copied image is refused before attachment'
            Assert-True ((Get-FileHash -LiteralPath $configPath).Hash -eq $configHash) 'binding checks leave the existing config bytes unchanged'
            Assert-True ((Get-FileHash -LiteralPath $copiedPath).Hash -eq $sourceHash) 'read-only copy inspection and refusal leave all image bytes unchanged'
            $images[0].VerifySavedBinding($saved.state_binding.backing_file_identity,
                [Guid]$saved.state_binding.virtual_disk_id)
            $bytes = (Get-Item -LiteralPath $images[0].Path).Length + (Get-Item -LiteralPath $copiedPath).Length
            Assert-True ($bytes -le 512MB) 'only two bounded image slots exist during copy refusal'
        }
    }
}
catch { $failure = $_.Exception.ToString() }
finally {
    foreach ($root in $roots) {
        try { $root.Dispose() } catch { $cleanupErrors.Add($_.Exception.ToString()) }
    }
    foreach ($image in $images) {
        try {
            $image.Detach()
            Assert-True (-not $image.Attached) 'exact generated image detached'
            $image.Dispose()
            [IO.File]::Delete($image.Path)
            Assert-True (-not [IO.File]::Exists($image.Path)) 'exact generated image removed'
        }
        catch { $cleanupErrors.Add($_.Exception.ToString()) }
    }
    if ($copiedPath -and [IO.File]::Exists($copiedPath)) {
        try {
            $current = [StorageWindowsVolumes]::InspectDetachedCopy($copiedPath)
            if (-not $copyObservation -or $current.Loaded -or
                $current.FileIdentity -ne $copyObservation.FileIdentity) {
                throw 'Copied image cleanup ownership was not proved.'
            }
            [IO.File]::Delete($copiedPath)
            Assert-True (-not [IO.File]::Exists($copiedPath)) 'exact detached copied image removed'
        } catch { $cleanupErrors.Add($_.Exception.ToString()) }
    }
    if ($configCreated -and [IO.File]::Exists($configPath)) {
        try { [IO.File]::Delete($configPath) }
        catch { $cleanupErrors.Add($_.Exception.ToString()) }
    }
    if ($cleanupErrors.Count -eq 0 -and [IO.Directory]::Exists($fixture)) {
        try { [IO.Directory]::Delete($fixture, $false) }
        catch { $cleanupErrors.Add($_.Exception.ToString()) }
    }
    $parentRoot.Dispose()
    $result = [ordered]@{
        result = $(if ($failure -or $cleanupErrors.Count) { 'failed' }
            elseif ($interrupted) { 'interrupted_cleanup_passed' }
            elseif ($SavedBindingProof) { 'saved_vhd_binding_diagnostic_passed' }
            else { 'native_volume_observations_measured' })
        assertions = @($assertions.ToArray())
        observations = @($observations.ToArray())
        generated_images = @($ownership.ToArray())
        host_private_candidate = $hostObservation
        saved_binding = $savedBinding
        reattached_observation = $reattached
        detached_copy_observation = $copyObservation
        backing_file_bytes = $bytes
        error = $failure
        cleanup_errors = @($cleanupErrors.ToArray())
        scope = 'Generated VHDX/NTFS observations. Different volume admission, durable identity, physical backing separation and production I/O remain unproved.'
        source_hashes = [ordered]@{
            root_collector = (Get-FileHash (Join-Path $PSScriptRoot 'storage-windows-prototype.cs')).Hash
            volume_collector = (Get-FileHash (Join-Path $PSScriptRoot 'storage-windows-volumes.cs')).Hash
            harness = (Get-FileHash $PSCommandPath).Hash
        }
    }
    $result | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $resultPath -Encoding UTF8
}
if ($failure -or $cleanupErrors.Count) { throw "Generated volume proof failed. See $resultPath" }

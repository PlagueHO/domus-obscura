[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
if (-not $IsWindows) {
    throw 'This isolated experiment requires Windows and PowerShell 7.'
}
Add-Type -LiteralPath (Join-Path $PSScriptRoot 'storage-windows-prototype.cs')
$fixture = Join-Path ([System.IO.Path]::GetTempPath()) "domus-storage-windows-$([Guid]::NewGuid().ToString('N'))"
$roots = [System.Collections.Generic.List[System.IDisposable]]::new()
$assertions = [System.Collections.Generic.List[string]]::new()
$junctionCreated = $false

function Assert-Relationship {
    param($Left, $Right, [string] $Expected, [string] $Label)
    $actual = [StorageWindowsPrototype]::Relationship($Left, $Right)
    if ($actual -ne $Expected) {
        throw "$Label returned $actual; expected $Expected."
    }
    $assertions.Add($Label)
}

function Assert-Rejected {
    param([scriptblock] $Action, [string] $Label, [string] $Diagnostic)
    $failed = $false
    try { & $Action | Out-Null }
    catch {
        $failed = $true
        if ($Diagnostic -and $_.Exception.ToString() -notmatch $Diagnostic) {
            throw "$Label failed with a different diagnostic: $($_.Exception.Message)"
        }
    }
    if (-not $failed) { throw "$Label was unexpectedly accepted." }
    $assertions.Add($Label)
}

function Open-Root {
    param([string] $Path)
    $root = [StorageWindowsPrototype]::Open($Path)
    $roots.Add($root)
    return $root
}

try {
    foreach ($child in @('private\staging', 'media', 'media-extra', 'replaceable')) {
        [System.IO.Directory]::CreateDirectory((Join-Path $fixture $child)) | Out-Null
    }
    [System.IO.File]::WriteAllText((Join-Path $fixture 'replaceable\owner.lock'), 'generated marker')
    $boundary = Open-Root $fixture
    $private = Open-Root (Join-Path $fixture 'private')
    $staging = Open-Root (Join-Path $fixture 'private\staging')
    $media = Open-Root (Join-Path $fixture 'media')
    $lookalike = Open-Root (Join-Path $fixture 'media-extra')
    foreach ($root in @($boundary, $private, $staging, $media, $lookalike)) {
        $root.Recheck()
        if ($root.Evidence.Identity -notmatch '^win128:[a-f0-9]{16}:[a-f0-9]{32}$') {
            throw 'The collector did not return the full Windows identity.'
        }
    }
    Assert-Relationship $private.Evidence $media.Evidence 'Separated' 'local NTFS siblings'
    Assert-Relationship $media.Evidence $lookalike.Evidence 'Separated' 'component prefix lookalike'
    Assert-Relationship $private.Evidence $staging.Evidence 'Overlapping' 'parent and child'
    Assert-Relationship $staging.Evidence $private.Evidence 'Overlapping' 'reversed nesting'
    Assert-Relationship $boundary.Evidence $media.Evidence 'Overlapping' 'media beneath private namespace'

    $guidAlias = Open-Root $staging.Evidence.Coordinate
    Assert-Relationship $staging.Evidence $guidAlias.Evidence 'Overlapping' 'drive and volume GUID aliases'
    if ($staging.Evidence.Identity -ne $guidAlias.Evidence.Identity -or $staging.Evidence.Coordinate -ne $guidAlias.Evidence.Coordinate) {
        throw 'Drive and volume GUID openings did not resolve to the same retained root.'
    }
    $caseAlias = Open-Root (Join-Path $fixture 'PRIVATE\STAGING')
    Assert-Relationship $staging.Evidence $caseAlias.Evidence 'Overlapping' 'case alias'
    $child = $private.Child('staging')
    if ($child.Identity -ne $staging.Evidence.Identity) { throw 'Retained child identity changed.' }
    $assertions.Add('retained parent-relative child')

    $junction = Join-Path $fixture 'junction'
    New-Item -ItemType Junction -Path $junction -Target (Join-Path $fixture 'private') | Out-Null
    $junctionCreated = $true
    Assert-Rejected { [StorageWindowsPrototype]::Open($junction).Dispose() } 'root junction' 'Reparse points'
    Assert-Rejected { [StorageWindowsPrototype]::Open((Join-Path $junction 'staging')).Dispose() } 'intermediate junction' 'Reparse points'
    Assert-Rejected { $boundary.Child('junction') } 'retained child junction' 'Reparse points'
    Assert-Rejected { [StorageWindowsPrototype]::Open((Join-Path $fixture 'missing')).Dispose() } 'missing root' ''
    Assert-Rejected { $private.Child('..\media') } 'child parent escape' 'Invalid single directory component'
    Assert-Rejected { $private.Child('staging:stream') } 'alternate stream' 'Invalid single directory component'

    $replaceable = Open-Root (Join-Path $fixture 'replaceable')
    $original = $replaceable.Evidence.Identity
    [System.IO.Directory]::Move((Join-Path $fixture 'replaceable'), (Join-Path $fixture 'retained'))
    [System.IO.Directory]::CreateDirectory((Join-Path $fixture 'replaceable')) | Out-Null
    [System.IO.File]::WriteAllText((Join-Path $fixture 'replaceable\owner.lock'), 'generated marker')
    if ($replaceable.ReadRetained().Identity -ne $original) { throw 'Retained handle followed the replacement.' }
    Assert-Rejected { $replaceable.Recheck() } 'replacement with copied marker' 'Retained root or configured path changed'

    # These are provider-evidence models, not connections to SMB servers.
    $remote = [StorageWindowsPrototype+Evidence]::new()
    $remote.Identity = $private.Evidence.Identity
    $remote.Coordinate = $private.Evidence.Coordinate
    $remote.Filesystem = 'NTFS'
    $remote.Remote = $true
    Assert-Relationship $private.Evidence $remote 'UnknownRemote' 'remote cannot borrow local NTFS evidence'
    Assert-Relationship $remote $remote 'UnknownRemote' 'matching remote IDs do not grant authority'
    $other = [StorageWindowsPrototype+Evidence]::new()
    $other.Identity = $media.Evidence.Identity
    $other.Coordinate = '\\?\Volume{11111111-2222-3333-4444-555555555555}\media'
    $other.Filesystem = 'NTFS'
    Assert-Relationship $private.Evidence $other 'UnknownVolume' 'different volume is not automatic separation'
    $other.Coordinate = '\\server\share\media'
    Assert-Relationship $private.Evidence $other 'UnknownCoordinate' 'UNC spelling is not a backing coordinate'
    $other.Coordinate = $media.Evidence.Coordinate
    $other.Filesystem = 'ReFS'
    Assert-Relationship $private.Evidence $other 'UnknownFilesystem' 'unverified filesystem'
    $other.Filesystem = 'NTFS'
    $other.Coordinate = $media.Evidence.Coordinate + '\..\private'
    Assert-Relationship $private.Evidence $other 'UnknownCoordinate' 'malformed backing coordinate'
    $other.Coordinate = $media.Evidence.Coordinate + '\non-ascii-' + [char]0xe9
    Assert-Relationship $private.Evidence $other 'UnknownCoordinate' 'unverified Unicode coordinate'
    $other.Coordinate = $media.Evidence.Coordinate
    $other.Identity = $null
    Assert-Relationship $private.Evidence $other 'UnknownIdentity' 'missing full identity'
    $other.Identity = 'short-file-id'
    Assert-Relationship $private.Evidence $other 'UnknownIdentity' 'short file ID cannot substitute'

    [ordered]@{
        result = 'restricted_windows_replacement_passed'
        filesystem = $private.Evidence.Filesystem
        assertions = @($assertions)
        source_hashes = [ordered]@{
            collector = (Get-FileHash (Join-Path $PSScriptRoot 'storage-windows-prototype.cs') -Algorithm SHA256).Hash
            harness = (Get-FileHash $PSCommandPath -Algorithm SHA256).Hash
        }
        evidence_scope = 'Generated local NTFS directories only. Actual retained handles, volume GUID alias, junctions and replacement. Remote/different-volume rejection is modeled, not live SMB acceptance. No production binding or host configuration changed.'
    } | ConvertTo-Json -Depth 5
}
finally {
    foreach ($root in $roots) { $root.Dispose() }
    if ($junctionCreated) {
        [System.IO.Directory]::Delete((Join-Path $fixture 'junction'), $false)
    }
    if ([System.IO.Directory]::Exists($fixture)) {
        [System.IO.Directory]::Delete($fixture, $true)
    }
}

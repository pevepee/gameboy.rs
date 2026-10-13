# Fetch the Blargg and mooneye test ROM suites into test-roms/.
#
# The ROMs are third-party binaries and are not committed. This script is
# idempotent: running it again updates the clones in place.
#
# Usage: pwsh -File scripts/fetch-test-roms.ps1

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$dest = Join-Path $root 'test-roms'

New-Item -ItemType Directory -Force -Path $dest | Out-Null

function Sync-Repo {
    param([string]$Url, [string]$Name)

    $target = Join-Path $dest $Name

    if (Test-Path (Join-Path $target '.git')) {
        Write-Host "updating $Name"
        git -C $target fetch --depth 1 origin
        git -C $target reset --hard FETCH_HEAD
    } else {
        Write-Host "cloning $Name"
        git clone --depth 1 $Url $target
    }
}

Sync-Repo 'https://github.com/retrio/gb-test-roms.git' 'gb-test-roms'
Sync-Repo 'https://github.com/Gekkio/mooneye-test-suite.git' 'mooneye-test-suite'

$count = (Get-ChildItem -Recurse -Filter '*.gb' $dest).Count
Write-Host "ready: $count test ROMs under $dest"

# Rebuilds vendor/mocari: upstream mocari 0.4.1 plus every patches/mocari-<version>-*.patch.
#
# The crate is committed, so a fresh clone builds without running this. Run it
# after bumping -Version, or to prove that the committed copy is exactly the
# registry release plus `patches/`; the only expected diff is the patches.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts/vendor-mocari.ps1 [-Version 0.4.1] [-Force]
#
# The patches are applied in file-name order, so their names must stay sortable.
# See README.md "The four patches" for what each one fixes.

param(
    [string]$Version = "0.4.1",
    [switch]$Force
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$dest = Join-Path $root "vendor/mocari"
$patchDir = Join-Path $root "patches"

if ((Test-Path $dest) -and -not $Force) {
    Write-Output "$dest is committed - pass -Force to rebuild it from the registry"
    exit 0
}

$registry = Join-Path $env:USERPROFILE ".cargo/registry/src"
if (-not (Test-Path $registry)) {
    throw "no cargo registry at $registry - run 'cargo fetch' first"
}

$candidates = Get-ChildItem $registry -Directory |
    ForEach-Object { Join-Path $_.FullName "mocari-$Version" } |
    Where-Object { Test-Path $_ }

if (-not $candidates) {
    throw "mocari-$Version is not in the cargo registry - run 'cargo fetch' first"
}

$source = @($candidates)[0]
Write-Output "source: $source"

if (Test-Path $dest) { Remove-Item -Recurse -Force $dest }
Copy-Item -Recurse $source $dest

# Files under the registry carry the read-only attribute; clear it before editing.
Get-ChildItem $dest -Recurse -File | ForEach-Object { $_.IsReadOnly = $false }

Push-Location $root
try {
    Get-ChildItem $patchDir -Filter "mocari-$Version-*.patch" | Sort-Object Name | ForEach-Object {
        git apply -p1 --directory=vendor/mocari $_.FullName
        if ($LASTEXITCODE -ne 0) { throw "git apply failed for $($_.Name)" }
        Write-Output "patched: $($_.Name)"
    }
}
finally {
    Pop-Location
}

Write-Output "done: $dest"

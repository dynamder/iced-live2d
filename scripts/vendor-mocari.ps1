# Rebuilds .refs/mocari: upstream mocari 0.4.1 plus our local fix.
#
# The workspace `[patch.crates-io]` in Cargo.toml points at .refs/mocari, so a
# fresh checkout has to run this once before `cargo build` can resolve mocari.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts/vendor-mocari.ps1
#
# Why a vendored crate instead of a fork on a git remote: the fix is three lines
# and the project should not grow a hard dependency on somebody's fork. See
# patches/mocari-0.4.1-negative-masks.patch for the reasoning.

param(
    [string]$Version = "0.4.1",
    [switch]$Force
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$dest = Join-Path $root ".refs/mocari"
$patchDir = Join-Path $root "patches"

if ((Test-Path $dest) -and -not $Force) {
    Write-Output "$dest already exists (pass -Force to rebuild)"
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

Push-Location $dest
try {
    git init --quiet .
    Get-ChildItem $patchDir -Filter "mocari-$Version-*.patch" | Sort-Object Name | ForEach-Object {
        git apply $_.FullName
        if ($LASTEXITCODE -ne 0) { throw "git apply failed for $($_.Name)" }
        Write-Output "patched: $($_.Name)"
    }
}
finally {
    Pop-Location
}

Write-Output "done: $dest"

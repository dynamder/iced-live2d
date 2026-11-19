# Bundles the probe page against the official Cubism Web SDK sources.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts/web-probe/build.ps1
#
# The SDK is not vendored: point -Sdk (or CUBISM_SDK) at a CubismSdkForWeb-*-r.*
# checkout. esbuild is the only build dependency; pass -Esbuild if it is not on
# PATH.

param(
    [string]$Sdk = '',
    [string]$Esbuild = ''
)

$here = $PSScriptRoot
$repo = Split-Path -Parent (Split-Path -Parent $here)

if ($Sdk -eq '') {
    $Sdk = if ($env:CUBISM_SDK) { $env:CUBISM_SDK } else { Join-Path $repo '.refs\cubism-web-sdk\CubismSdkForWeb-5-r.4' }
}

$framework = Join-Path $Sdk 'Framework\src'

if (-not (Test-Path $framework)) {
    Write-Error "no Cubism Framework sources at $framework - pass -Sdk or set CUBISM_SDK"
    exit 1
}

if ($Esbuild -eq '') { $Esbuild = 'esbuild' }

& $Esbuild (Join-Path $here 'main.ts') --bundle --format=esm --target=es2022 `
    "--alias:@framework=$framework" `
    "--outfile=$(Join-Path $here 'main.js')"

if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Output "built $(Join-Path $here 'main.js') against $framework"

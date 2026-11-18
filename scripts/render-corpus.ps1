# Renders every model of the test corpus to a PNG, so the images can be judged by
# eye. Assets stay under .refs/ and .tmp-render/; neither is committed.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts/render-corpus.ps1
#   ... -Size 640 -Steps 60 -TextureScale 1.0 -Only Hiyori,Wanko -Sheet
#
# Every model is rendered with the same knobs, which makes the PNGs comparable
# side by side. -Sheet additionally stitches them into .tmp-render/corpus.png.

param(
    [int]$Size = 400,
    [int]$Steps = 30,
    [double]$TextureScale = 0.5,
    [string]$Only = "",
    [switch]$Sheet,
    [string]$Out = ""
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$resources = Join-Path $root ".refs/cubism-web-samples/Samples/Resources"
$example = Join-Path $root "target/release/examples/offscreen.exe"
$outDir = if ($Out) { $Out } else { Join-Path $root ".tmp-render" }

if (-not (Test-Path $example)) {
    throw "missing $example - run: cargo build --release --examples -p live2d-render"
}

if (-not (Test-Path $resources)) {
    throw "missing $resources - clone the samples first (see docs/test-models.md)"
}

New-Item -ItemType Directory -Force -Path $outDir | Out-Null

$filter = @()
if ($Only) { $filter = $Only.Split(",") | ForEach-Object { $_.Trim() } }

$models = Get-ChildItem $resources -Directory | Where-Object {
    $filter.Count -eq 0 -or $filter -contains $_.Name
}

if (-not $models) {
    throw "no model matched '$Only' under $resources"
}

$env:LIVE2D_TEXTURE_SCALE = $TextureScale.ToString([System.Globalization.CultureInfo]::InvariantCulture)

$rendered = @()
foreach ($model in $models) {
    $json = Join-Path $model.FullName "$($model.Name).model3.json"
    if (-not (Test-Path $json)) {
        Write-Output "skip $($model.Name): no $($model.Name).model3.json"
        continue
    }

    $png = Join-Path $outDir "$($model.Name).png"
    Write-Output "--- $($model.Name) ---"
    & $example $json $png $Size $Steps
    if ($LASTEXITCODE -ne 0) {
        Write-Output "FAILED $($model.Name) (exit $LASTEXITCODE)"
        continue
    }
    $rendered += $model.Name
}

Write-Output ""
Write-Output "rendered $($rendered.Count) model(s) into $outDir"

if ($Sheet -and $rendered.Count -gt 0) {
    $python = "C:\Users\atomt\.dsh\dsh-runtimes\dsh-primary-runtime\dependencies\python\python.exe"
    $sheetter = Join-Path $PSScriptRoot "contact-sheet.py"
    & $python $sheetter $outDir
}

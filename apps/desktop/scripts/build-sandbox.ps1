$ErrorActionPreference = 'Stop'

$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$image = if ([string]::IsNullOrWhiteSpace($env:DRAY_CLOUD_IMAGE)) {
    'lathe-cloud:latest'
} else {
    $env:DRAY_CLOUD_IMAGE
}
Write-Host "Building Lathe Cloud sandbox image $image"
& docker build `
    --file (Join-Path $root 'sandbox/Dockerfile') `
    --tag $image `
    (Resolve-Path (Join-Path $root '../..')).Path

if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}

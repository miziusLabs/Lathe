# Regenerate the bundled icons from the 1024px masters in src-tauri/icons/src.
# Run after replacing a master; the outputs are committed, so this is not part
# of any build.
$ErrorActionPreference = 'Stop'

$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Set-Location $root
$src = Join-Path $root 'src-tauri/icons/src'
$out = Join-Path $root 'src-tauri/icons'

# Masters may include transparent canvas padding; trim it before resizing.
# macOS sizes dock icons off a 824/1024 grid, so an icns built from full-bleed
# art draws visibly larger than every neighbour. Inset for the icns only - the
# flat PNGs are the window/Linux/tray icon, where full-bleed is right.
$inset = 824

# `tauri::generate_context!` rejects a window icon that isn't RGBA, and magick
# happily writes a palette PNG when the resized art has few enough colours - so
# every PNG is forced to 32-bit rather than left to that heuristic.
$png32 = 'PNG32:'

function Invoke-External {
    param(
        [Parameter(Mandatory = $true)]
        [string] $Command,
        [Parameter(Mandatory = $false)]
        [string[]] $Arguments = @()
    )

    & $Command @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$Command failed with exit code $LASTEXITCODE"
    }
}

function New-Icns {
    param(
        [Parameter(Mandatory = $true)]
        [string] $Source,
        [Parameter(Mandatory = $true)]
        [string] $Destination
    )

    $temp = Join-Path ([System.IO.Path]::GetTempPath()) ([Guid]::NewGuid().ToString())
    $iconset = Join-Path $temp 'icon.iconset'
    New-Item -ItemType Directory -Path $iconset -Force | Out-Null

    try {
        $master = Join-Path $temp 'master.png'
        Invoke-External 'magick' @(
            $Source
            '-trim'
            '+repage'
            '-resize'
            "${inset}x${inset}"
            '-background'
            'none'
            '-gravity'
            'center'
            '-extent'
            '1024x1024'
            $master
        )

        $sizes = @(
            @{ Pixels = 16; Name = '16x16' }
            @{ Pixels = 32; Name = '16x16@2x' }
            @{ Pixels = 32; Name = '32x32' }
            @{ Pixels = 64; Name = '32x32@2x' }
            @{ Pixels = 128; Name = '128x128' }
            @{ Pixels = 256; Name = '128x128@2x' }
            @{ Pixels = 256; Name = '256x256' }
            @{ Pixels = 512; Name = '256x256@2x' }
            @{ Pixels = 512; Name = '512x512' }
            @{ Pixels = 1024; Name = '512x512@2x' }
        )

        foreach ($size in $sizes) {
            $destination = Join-Path $iconset "icon_$($size.Name).png"
            Invoke-External 'magick' @(
                $master
                '-resize'
                "$($size.Pixels)x$($size.Pixels)"
                ($png32 + $destination)
            )
        }

        Invoke-External 'iconutil' @('-c', 'icns', $iconset, '-o', $Destination)
    } finally {
        if (Test-Path -LiteralPath $temp) {
            Remove-Item -LiteralPath $temp -Recurse -Force
        }
    }
}

function New-FlatIcons {
    param(
        [Parameter(Mandatory = $true)]
        [string] $Source,
        [Parameter(Mandatory = $true)]
        [string] $Directory
    )

    New-Item -ItemType Directory -Path $Directory -Force | Out-Null
    Invoke-External 'magick' @($Source, '-trim', '+repage', '-resize', '32x32', ($png32 + (Join-Path $Directory '32x32.png')))
    Invoke-External 'magick' @($Source, '-trim', '+repage', '-resize', '128x128', ($png32 + (Join-Path $Directory '128x128.png')))
    Invoke-External 'magick' @($Source, '-trim', '+repage', '-resize', '256x256', ($png32 + (Join-Path $Directory '128x128@2x.png')))
    Invoke-External 'magick' @($Source, '-trim', '+repage', '-resize', '1024x1024', ($png32 + (Join-Path $Directory 'icon.png')))
}

New-FlatIcons (Join-Path $src 'icon-1024.png') $out
New-Icns (Join-Path $src 'icon-1024.png') (Join-Path $out 'icon.icns')
Invoke-External 'magick' @(
    (Join-Path $src 'icon-1024.png')
    '-trim'
    '+repage'
    '-define'
    'icon:auto-resize=256,128,64,48,32,16'
    (Join-Path $out 'icon.ico')
)

$dev = Join-Path $out 'dev'
New-FlatIcons (Join-Path $src 'icon-dev-1024.png') $dev
New-Icns (Join-Path $src 'icon-dev-1024.png') (Join-Path $dev 'icon.icns')
Invoke-External 'magick' @(
    (Join-Path $src 'icon-dev-1024.png')
    '-trim'
    '+repage'
    '-define'
    'icon:auto-resize=256,128,64,48,32,16'
    (Join-Path $dev 'icon.ico')
)

Write-Host 'icons regenerated'

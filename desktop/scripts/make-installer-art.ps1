# Draws the Windows installer's artwork from the app icon and the @kahf/design colour tokens:
#   src-tauri/windows/art/sidebar.bmp  welcome and finish pages, 2x of NSIS's 164x314
#   src-tauri/windows/art/header.bmp   the other pages' header, 2x of NSIS's 150x57
# NSIS scales them to the page, so the 2x size keeps them sharp at 200 % display scaling.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File desktop/scripts/make-installer-art.ps1

Add-Type -AssemblyName System.Drawing
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$tauri = Join-Path $here '..\src-tauri'
$out = Join-Path $tauri 'windows\art'
New-Item -ItemType Directory -Force $out | Out-Null
$icon = [System.Drawing.Image]::FromFile((Resolve-Path (Join-Path $tauri 'icons\icon.png')))

# Tokens: --kf-product-kahf (Wasl's primary), its container, --md-sys-color-surface-bright.
function C([string] $hex) { [System.Drawing.ColorTranslator]::FromHtml($hex) }
$primary = C '#1f3d6b'
$primaryLight = C '#2f5a96'
$container = C '#e4ebf6'
$surface = C '#fbfaf8'

function New-Canvas([int] $w, [int] $h) {
    $bmp = New-Object System.Drawing.Bitmap $w, $h, ([System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = 'AntiAlias'
    $g.InterpolationMode = 'HighQualityBicubic'
    $g.TextRenderingHint = 'AntiAliasGridFit'
    @($bmp, $g)
}

# Sidebar: the primary colour, a soft light from the top, the icon on a container-coloured disc.
$bmp, $g = New-Canvas 328 628
$rect = New-Object System.Drawing.Rectangle 0, 0, 328, 628
$grad = New-Object System.Drawing.Drawing2D.LinearGradientBrush $rect, $primaryLight, $primary, 90
$g.FillRectangle($grad, $rect)
$ring = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(40, 255, 255, 255)), 2
foreach ($r in 150, 210, 270) { $g.DrawEllipse($ring, 164 - $r, 210 - $r, 2 * $r, 2 * $r) }
$g.FillEllipse((New-Object System.Drawing.SolidBrush $container), 84, 130, 160, 160)
$g.DrawImage($icon, 104, 150, 120, 120)
$font = New-Object System.Drawing.Font 'Segoe UI Semibold', 44, ([System.Drawing.FontStyle]::Regular), ([System.Drawing.GraphicsUnit]::Pixel)
$fmt = New-Object System.Drawing.StringFormat
$fmt.Alignment = 'Center'
$g.DrawString('Wasl', $font, [System.Drawing.Brushes]::White, (New-Object System.Drawing.RectangleF 0, 320, 328, 60), $fmt)
$small = New-Object System.Drawing.Font 'Segoe UI', 20, ([System.Drawing.FontStyle]::Regular), ([System.Drawing.GraphicsUnit]::Pixel)
$g.DrawString('Kahf Labs', $small, (New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(200, 255, 255, 255))), (New-Object System.Drawing.RectangleF 0, 382, 328, 30), $fmt)
$g.Dispose()
$bmp.Save((Join-Path $out 'sidebar.bmp'), [System.Drawing.Imaging.ImageFormat]::Bmp)
$bmp.Dispose()

# Header: the page's own light surface with the icon, so the title text beside it stays readable.
$bmp, $g = New-Canvas 300 114
$g.Clear($surface)
$g.FillEllipse((New-Object System.Drawing.SolidBrush $container), 196, 17, 80, 80)
$g.DrawImage($icon, 206, 27, 60, 60)
$g.Dispose()
$bmp.Save((Join-Path $out 'header.bmp'), [System.Drawing.Imaging.ImageFormat]::Bmp)
$bmp.Dispose()
$icon.Dispose()
Write-Output "wrote $out\sidebar.bmp and header.bmp"

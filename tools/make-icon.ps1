# Generates assets\battery-tray.ico: a battery with a green level, drawn per size so small sizes stay crisp.
# Usage: pwsh -File tools\make-icon.ps1   (optionally -Preview to also write a PNG contact sheet)
param([switch]$Preview)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

$root = Split-Path $PSScriptRoot -Parent
$out = Join-Path $root 'assets\battery-tray.ico'
New-Item -ItemType Directory -Force (Split-Path $out) | Out-Null

function RoundRect([float]$x, [float]$y, [float]$w, [float]$h, [float]$r) {
    $p = [System.Drawing.Drawing2D.GraphicsPath]::new()
    $d = [Math]::Min($r * 2, [Math]::Min($w, $h))
    if ($d -le 0) { $p.AddRectangle([System.Drawing.RectangleF]::new($x, $y, $w, $h)); return $p }
    $p.AddArc($x, $y, $d, $d, 180, 90)
    $p.AddArc($x + $w - $d, $y, $d, $d, 270, 90)
    $p.AddArc($x + $w - $d, $y + $h - $d, $d, $d, 0, 90)
    $p.AddArc($x, $y + $h - $d, $d, $d, 90, 90)
    $p.CloseFigure()
    $p
}

function Draw([int]$s) {
    $bmp = [System.Drawing.Bitmap]::new($s, $s, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = 'AntiAlias'
    $g.PixelOffsetMode = 'HighQuality'
    $g.Clear([System.Drawing.Color]::Transparent)

    $u = $s / 32.0                              # design grid is 32x32
    $stroke = [Math]::Max(1.0, [Math]::Round(1.6 * $u))
    $body = [System.Drawing.Color]::FromArgb(255, 0x2E, 0x3B, 0x47)     # dark slate: reads on light backgrounds
    $edge = [System.Drawing.Color]::FromArgb(255, 0xA9, 0xBC, 0xCC)     # light rim: reads on dark backgrounds
    $green = [System.Drawing.Color]::FromArgb(255, 0x43, 0xC5, 0x7E)

    # Body, snapped to whole pixels at small sizes.
    $bx = [Math]::Round(1.5 * $u); $by = [Math]::Round(8 * $u)
    $bw = [Math]::Round(26 * $u);  $bh = [Math]::Round(16 * $u)
    $r = 3.2 * $u
    $half = $stroke / 2
    $path = RoundRect ($bx + $half) ($by + $half) ($bw - $stroke) ($bh - $stroke) $r
    $g.FillPath([System.Drawing.SolidBrush]::new($body), $path)
    $g.DrawPath([System.Drawing.Pen]::new($edge, $stroke), $path)

    # Terminal nub.
    $nw = [Math]::Max(1, [Math]::Round(2.5 * $u)); $nh = [Math]::Round(7 * $u)
    $nub = RoundRect ($bx + $bw) ($by + ($bh - $nh) / 2) $nw $nh ([Math]::Min(1.2 * $u, $nw / 2))
    $g.FillPath([System.Drawing.SolidBrush]::new($edge), $nub)

    # Level fill (about 70%), split into segments when there's room.
    $inset = $stroke + [Math]::Max(1, [Math]::Round(1.4 * $u))
    $fx = $bx + $inset; $fy = $by + $inset
    $fw = ($bw - 2 * $inset); $fh = ($bh - 2 * $inset)
    $level = 0.7
    $brush = [System.Drawing.SolidBrush]::new($green)
    if ($s -ge 32) {
        $segs = 4; $gap = [Math]::Max(1, [Math]::Round(1.2 * $u))
        $segw = [Math]::Floor(($fw - ($segs - 1) * $gap) / $segs)
        $fullSegs = [Math]::Round($level * $segs - 0.2)
        for ($i = 0; $i -lt $fullSegs; $i++) {
            $seg = RoundRect ([Math]::Round($fx + $i * ($segw + $gap))) $fy $segw $fh (0.9 * $u)
            $g.FillPath($brush, $seg)
        }
    } else {
        $g.FillRectangle($brush, [float]$fx, [float]$fy, [float][Math]::Round($fw * $level), [float]$fh)
    }
    $g.Dispose()
    $bmp
}

$sizes = 16, 20, 24, 32, 40, 48, 64, 256
$images = foreach ($s in $sizes) {
    $bmp = Draw $s
    $ms = [System.IO.MemoryStream]::new()
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    [pscustomobject]@{ Size = $s; Bytes = $ms.ToArray(); Bitmap = $bmp }
}

# ICO container with PNG-encoded images (supported since Windows Vista).
$ms = [System.IO.MemoryStream]::new()
$w = [System.IO.BinaryWriter]::new($ms)
$w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$images.Count)
$offset = 6 + 16 * $images.Count
foreach ($img in $images) {
    $dim = if ($img.Size -ge 256) { 0 } else { $img.Size }
    $w.Write([byte]$dim); $w.Write([byte]$dim); $w.Write([byte]0); $w.Write([byte]0)
    $w.Write([uint16]1); $w.Write([uint16]32)
    $w.Write([uint32]$img.Bytes.Length); $w.Write([uint32]$offset)
    $offset += $img.Bytes.Length
}
foreach ($img in $images) { $w.Write($img.Bytes) }
$w.Flush()
[System.IO.File]::WriteAllBytes($out, $ms.ToArray())
"wrote $out ($($ms.Length) bytes)"

if ($Preview) {
    # Each size at 4x (nearest neighbour), on dark and light backgrounds.
    $cell = 136; $sheet = [System.Drawing.Bitmap]::new($cell * $sizes.Count, $cell * 2 + 260 * 2)
    $g = [System.Drawing.Graphics]::FromImage($sheet)
    $g.InterpolationMode = 'NearestNeighbor'; $g.PixelOffsetMode = 'Half'
    $bgs = [System.Drawing.Color]::FromArgb(255, 0x1F, 0x1F, 0x1F), [System.Drawing.Color]::FromArgb(255, 0xF3, 0xF3, 0xF3)
    for ($row = 0; $row -lt 2; $row++) {
        $g.FillRectangle([System.Drawing.SolidBrush]::new($bgs[$row]), 0, $row * $cell, $sheet.Width, $cell)
        for ($i = 0; $i -lt $sizes.Count - 1; $i++) {
            $img = $images[$i]; $z = [Math]::Min(4, [Math]::Floor(128 / $img.Size))
            $g.DrawImage($img.Bitmap, $i * $cell + 4, $row * $cell + 4, $img.Size * $z, $img.Size * $z)
        }
        $g.FillRectangle([System.Drawing.SolidBrush]::new($bgs[$row]), 0, 2 * $cell + $row * 260, $sheet.Width, 260)
        $g.DrawImage($images[-1].Bitmap, 2, 2 * $cell + $row * 260 + 2, 256, 256)
    }
    $g.Dispose()
    $png = Join-Path $root 'assets\icon-preview.png'
    $sheet.Save($png, [System.Drawing.Imaging.ImageFormat]::Png)
    "wrote $png"
}

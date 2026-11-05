# Erzeugt speichermonitor.ico (16-256 px, PNG-Einträge) und icon64.rgba (Fenster-Icon).
# Aufruf im Projektordner: powershell -NoProfile -File werkzeug\icon.ps1
Add-Type -AssemblyName System.Drawing
$ziel = Split-Path $PSScriptRoot

function Bild([int]$n) {
    $b = New-Object System.Drawing.Bitmap $n, $n
    $g = [System.Drawing.Graphics]::FromImage($b)
    $g.SmoothingMode = 'AntiAlias'
    $g.Clear([System.Drawing.Color]::Transparent)
    $s = $n / 16.0
    $tuerkis = [System.Drawing.Color]::FromArgb(255, 0x5e, 0xea, 0xd4)
    $dunkel = [System.Drawing.Color]::FromArgb(255, 0x11, 0x18, 0x27)
    # Festplatte: dunkles Gehäuse mit türkisem Rand
    $g.FillRectangle((New-Object System.Drawing.SolidBrush $dunkel), 0.5 * $s, 2 * $s, 15 * $s, 12 * $s)
    $g.DrawRectangle([System.Drawing.Pen]::new($tuerkis, [float][Math]::Max(1, $s)), 0.5 * $s, 2 * $s, 15 * $s, 12 * $s)
    # Füllstandsbalken (gut zur Hälfte belegt) und Kontrollleuchte
    $g.DrawRectangle([System.Drawing.Pen]::new($tuerkis, [float][Math]::Max(1, $s * 0.6)), 2.5 * $s, 5 * $s, 11 * $s, 3 * $s)
    $g.FillRectangle((New-Object System.Drawing.SolidBrush $tuerkis), 2.5 * $s, 5 * $s, 7 * $s, 3 * $s)
    $g.FillEllipse((New-Object System.Drawing.SolidBrush $tuerkis), 11.5 * $s, 10 * $s, 2 * $s, 2 * $s)
    $g.Dispose()
    $b
}

$groessen = 16, 24, 32, 48, 64, 256
$pngs = foreach ($n in $groessen) {
    $ms = New-Object System.IO.MemoryStream
    (Bild $n).Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    , $ms.ToArray()
}
$out = New-Object System.IO.MemoryStream
$w = New-Object System.IO.BinaryWriter $out
$w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$groessen.Count)
$offset = 6 + 16 * $groessen.Count
for ($i = 0; $i -lt $groessen.Count; $i++) {
    $n = $groessen[$i] % 256  # 0 bedeutet 256
    $w.Write([byte]$n); $w.Write([byte]$n); $w.Write([byte]0); $w.Write([byte]0)
    $w.Write([uint16]1); $w.Write([uint16]32)
    $w.Write([uint32]$pngs[$i].Length); $w.Write([uint32]$offset)
    $offset += $pngs[$i].Length
}
foreach ($p in $pngs) { $w.Write($p) }
[IO.File]::WriteAllBytes((Join-Path $ziel 'speichermonitor.ico'), $out.ToArray())

# 64x64 RGBA roh für eframe (with_icon)
$b = Bild 64
$rgba = New-Object byte[] (64 * 64 * 4)
for ($y = 0; $y -lt 64; $y++) {
    for ($x = 0; $x -lt 64; $x++) {
        $c = $b.GetPixel($x, $y); $i = ($y * 64 + $x) * 4
        $rgba[$i] = $c.R; $rgba[$i + 1] = $c.G; $rgba[$i + 2] = $c.B; $rgba[$i + 3] = $c.A
    }
}
[IO.File]::WriteAllBytes((Join-Path $ziel 'icon64.rgba'), $rgba)

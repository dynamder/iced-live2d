# Captures the Live2D spike window to a PNG for visual verification.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts/shot.ps1 [-Process app] [-Out .tmp-shot.png]
#
# The window is moved to the top-left corner at its *current* physical size. Do
# not resize it here: on a scaled display the app renders a 1.5x larger surface
# and a hard-coded size would capture only the top-left corner of it.

param(
    [string]$Process = "app",
    [string]$Out = (Join-Path (Split-Path -Parent $PSScriptRoot) ".tmp-shot.png")
)

# Windows virtualizes GetWindowRect/CopyFromScreen for DPI-unaware processes,
# which silently crops captures of scaled windows. Ask for per-monitor v2 first.
Add-Type @'
using System;
using System.Runtime.InteropServices;
public class Dpi {
    [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
}
'@
[Dpi]::SetProcessDpiAwarenessContext([IntPtr](-4)) | Out-Null

Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public struct RECT { public int Left, Top, Right, Bottom; }
public class Win {
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool MoveWindow(IntPtr h, int x, int y, int w, int ht, bool repaint);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
}
'@

$proc = Get-Process $Process -ErrorAction SilentlyContinue |
    Where-Object { $_.MainWindowHandle -ne 0 } |
    Select-Object -First 1

if (-not $proc) {
    Write-Output "no window for process '$Process'"
    exit 1
}

$handle = $proc.MainWindowHandle
[Win]::ShowWindow($handle, 9) | Out-Null   # SW_RESTORE

$rect = New-Object RECT
[Win]::GetWindowRect($handle, [ref]$rect) | Out-Null
$width = $rect.Right - $rect.Left
$height = $rect.Bottom - $rect.Top

# Same physical size, just parked at the origin so the capture is faithful.
[Win]::MoveWindow($handle, 0, 0, $width, $height, $true) | Out-Null
[Win]::SetForegroundWindow($handle) | Out-Null
Start-Sleep -Milliseconds 900

[Win]::GetWindowRect($handle, [ref]$rect) | Out-Null
$width = $rect.Right - $rect.Left
$height = $rect.Bottom - $rect.Top

$bitmap = New-Object System.Drawing.Bitmap $width, $height
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$graphics.CopyFromScreen($rect.Left, $rect.Top, 0, 0, $bitmap.Size)
$bitmap.Save($Out)
$graphics.Dispose()
$bitmap.Dispose()

Write-Output "saved $Out (${width}x${height})"

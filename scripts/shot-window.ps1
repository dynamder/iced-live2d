# Captures any top-level window by title prefix, at its native physical size.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts/shot-window.ps1 -Title "Live2D Cubism Viewer" -Out .tmp-viewer.png
#
# The window is raised, parked at the origin and captured with CopyFromScreen.
# That works for OpenGL/Java windows (the Cubism Viewer is one) where
# PrintWindow usually returns a blank bitmap.

param(
    [Parameter(Mandatory = $true)][string]$Title,
    [string]$Out = "D:\native-cubism\.tmp-window.png",
    [switch]$NoMove
)

Add-Type @'
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public class WinShot {
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern int GetWindowTextLength(IntPtr h);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool MoveWindow(IntPtr h, int x, int y, int w, int ht, bool repaint);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
    public struct RECT { public int Left, Top, Right, Bottom; }

    public static IntPtr Find(string prefix) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((h, l) => {
            int len = GetWindowTextLength(h);
            if (len == 0 || !IsWindowVisible(h)) return true;
            var sb = new StringBuilder(len + 1);
            GetWindowText(h, sb, sb.Capacity);
            if (sb.ToString().StartsWith(prefix, StringComparison.OrdinalIgnoreCase)) {
                found = h;
                return false;
            }
            return true;
        }, IntPtr.Zero);
        return found;
    }

    public static string TitleOf(IntPtr h) {
        int len = GetWindowTextLength(h);
        var sb = new StringBuilder(len + 1);
        GetWindowText(h, sb, sb.Capacity);
        return sb.ToString();
    }
}
'@

[WinShot]::SetProcessDpiAwarenessContext([IntPtr](-4)) | Out-Null

$handle = [WinShot]::Find($Title)
if ($handle -eq [IntPtr]::Zero) {
    Write-Output "no visible window whose title starts with '$Title'"
    exit 1
}

Write-Output ("window: " + [WinShot]::TitleOf($handle))

[WinShot]::ShowWindow($handle, 9) | Out-Null    # SW_RESTORE
$rect = New-Object WinShot+RECT
[WinShot]::GetWindowRect($handle, [ref]$rect) | Out-Null
$width = $rect.Right - $rect.Left
$height = $rect.Bottom - $rect.Top

if (-not $NoMove) {
    [WinShot]::MoveWindow($handle, 0, 0, $width, $height, $true) | Out-Null
}

[WinShot]::SetForegroundWindow($handle) | Out-Null
Start-Sleep -Milliseconds 900

[WinShot]::GetWindowRect($handle, [ref]$rect) | Out-Null
$width = $rect.Right - $rect.Left
$height = $rect.Bottom - $rect.Top

Add-Type -AssemblyName System.Drawing
$bitmap = New-Object System.Drawing.Bitmap $width, $height
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$graphics.CopyFromScreen($rect.Left, $rect.Top, 0, 0, $bitmap.Size)
$bitmap.Save($Out)
$graphics.Dispose()
$bitmap.Dispose()

Write-Output "saved $Out (${width}x${height})"

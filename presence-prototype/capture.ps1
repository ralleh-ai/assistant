# Dev helper: screenshot the prototype window's client area. Not part of the
# crate.
#
# Two things here are load-bearing and both cost a debugging detour when they
# were wrong:
#
# 1. DPI awareness is declared before anything queries window or screen
#    geometry. In a DPI-unaware process on a scaled display, the geometry APIs
#    report logical pixels while `CopyFromScreen` copies physical ones, so the
#    capture silently becomes a magnified crop of the desktop's top-left
#    corner — which reads as a perfectly centred entity being off-centre.
#
# 2. The capture is the window's *client rect*, not the screen and not a
#    maximized window. `ShowWindow(SW_MAXIMIZE)` does not reliably resize an
#    already-running window here, and when it half-works the render surface and
#    the window disagree in size, leaving a black band that reads as the entity
#    being cropped. Framing the client rect makes the capture independent of
#    where and how large the window happens to be.
param(
    [string]$Out = "capture.png"
)

Add-Type @"
using System;
using System.Runtime.InteropServices;

[StructLayout(LayoutKind.Sequential)]
public struct RECT { public int Left, Top, Right, Bottom; }

[StructLayout(LayoutKind.Sequential)]
public struct POINT { public int X, Y; }

public static class Win {
    [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int w, int t, uint f);
    [DllImport("user32.dll")] public static extern bool SwitchToThisWindow(IntPtr h, bool alt);
}
"@

# DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 = -4; falls back on older builds.
if (-not [Win]::SetProcessDpiAwarenessContext([IntPtr](-4))) { [void][Win]::SetProcessDPIAware() }

Add-Type -AssemblyName System.Drawing

$proc = Get-Process -Name "presence-runtime" -ErrorAction SilentlyContinue
if (-not $proc) { Write-Error "presence-runtime is not running"; exit 1 }
$handle = $proc.MainWindowHandle

# HWND_TOPMOST (no move, no resize) so nothing occludes the capture.
[void][Win]::SetWindowPos($handle, [IntPtr](-1), 0, 0, 0, 0, 0x0001 -bor 0x0002)
[void][Win]::SwitchToThisWindow($handle, $true)
Start-Sleep -Seconds 2

$rect = New-Object RECT
if (-not [Win]::GetClientRect($handle, [ref]$rect)) { Write-Error "GetClientRect failed"; exit 1 }
$origin = New-Object POINT
if (-not [Win]::ClientToScreen($handle, [ref]$origin)) { Write-Error "ClientToScreen failed"; exit 1 }

$w = $rect.Right - $rect.Left
$h = $rect.Bottom - $rect.Top

$bmp = New-Object System.Drawing.Bitmap $w, $h
$gfx = [System.Drawing.Graphics]::FromImage($bmp)
$gfx.CopyFromScreen($origin.X, $origin.Y, 0, 0, (New-Object System.Drawing.Size $w, $h))
$bmp.Save([System.IO.Path]::GetFullPath([System.IO.Path]::Combine((Get-Location).Path, $Out)))
$gfx.Dispose()
$bmp.Dispose()
Write-Host "saved $Out (${w}x${h})"

using System.Runtime.InteropServices;
using System.Windows;
using System.Windows.Interop;
using System.Windows.Media;
using Dsa.Settings.Core;

namespace Dsa.Settings;

/// <summary>
/// Opens a window on the monitor the pointer is on (where WPF's CenterScreen puts it too), sized
/// to fit that monitor's work area and placed inside it, so the title bar never starts above
/// the top of the screen. <see cref="WindowFit"/> holds the arithmetic.
///
/// The work area and the monitor's DPI come from Windows in physical pixels, and the window is
/// placed in physical pixels too: with per-monitor DPI, WPF's Left and Top are in the DIPs of
/// whichever monitor the window is on at the time, which is not yet the one it opens on.
/// </summary>
internal static class StartupPlacement
{
    /// <summary>Places <paramref name="window"/> once its handle exists, before it is shown.
    /// Its XAML Width and Height are the preferred size; its MinWidth and MinHeight the least.</summary>
    public static void Attach(Window window)
    {
        window.WindowStartupLocation = WindowStartupLocation.Manual;
        var preferredWidth = window.Width;
        var preferredHeight = window.Height;
        window.SourceInitialized += (_, _) =>
        {
            try
            {
                Place(window, preferredWidth, preferredHeight);
            }
            catch (Exception e) when (e is DllNotFoundException or EntryPointNotFoundException)
            {
                // Without the monitor APIs the window keeps the place Windows gives it.
            }
        };
    }

    private static void Place(Window window, double preferredWidth, double preferredHeight)
    {
        var hwnd = new WindowInteropHelper(window).Handle;
        if (hwnd == IntPtr.Zero || !GetCursorPos(out var cursor))
        {
            return;
        }
        var monitor = MonitorFromPoint(cursor, MonitorDefaultToNearest);
        var info = new MonitorInfo { Size = Marshal.SizeOf<MonitorInfo>() };
        if (monitor == IntPtr.Zero || !GetMonitorInfo(monitor, ref info))
        {
            return;
        }
        if (GetDpiForMonitor(monitor, MdtEffectiveDpi, out var dpiX, out _) != 0 || dpiX == 0)
        {
            dpiX = 96;
        }
        var work = info.Work;
        var rect = WindowFit.Place(new PixelRect(work.Left, work.Top, work.Right - work.Left, work.Bottom - work.Top),
            dpiX, preferredWidth, preferredHeight, window.MinWidth, window.MinHeight);
        // The first move can carry the window onto a monitor with another DPI. WPF then rescales
        // it to the size Windows suggests for the new DPI, so the rectangle is set again once
        // the window is on the monitor it opens on, where the second move changes no DPI.
        for (var pass = 0; pass < 2; pass++)
        {
            SetWindowPos(hwnd, IntPtr.Zero, rect.Left, rect.Top, rect.Width, rect.Height,
                SwpNoZOrder | SwpNoActivate | SwpNoOwnerZOrder);
        }
        // Width and Height still hold the preferred size; they take the placed size, in the DIPs
        // of the monitor the window is now on, so no later layout pass restores the old one.
        var scale = VisualTreeHelper.GetDpi(window).PixelsPerDip;
        if (scale > 0)
        {
            window.Width = rect.Width / scale;
            window.Height = rect.Height / scale;
        }
    }

    private const uint MonitorDefaultToNearest = 2;
    private const int MdtEffectiveDpi = 0;
    private const uint SwpNoZOrder = 0x0004, SwpNoActivate = 0x0010, SwpNoOwnerZOrder = 0x0200;

    [StructLayout(LayoutKind.Sequential)]
    private struct Point
    {
        public int X, Y;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct Rect
    {
        public int Left, Top, Right, Bottom;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct MonitorInfo
    {
        public int Size;
        public Rect Monitor;
        public Rect Work;
        public uint Flags;
    }

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetCursorPos(out Point point);

    [DllImport("user32.dll")]
    private static extern IntPtr MonitorFromPoint(Point point, uint flags);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetMonitorInfo(IntPtr monitor, ref MonitorInfo info);

    [DllImport("shcore.dll")]
    private static extern int GetDpiForMonitor(IntPtr monitor, int type, out uint dpiX, out uint dpiY);

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool SetWindowPos(IntPtr hwnd, IntPtr insertAfter, int x, int y, int width, int height,
        uint flags);
}

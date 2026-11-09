using System.IO;
using System.Windows;
using System.Windows.Interop;
using System.Windows.Media;
using System.Windows.Threading;
using Dsa.Settings.Core;
using Microsoft.Win32;
using Wpf.Ui.Appearance;

namespace Dsa.Settings;

/// <summary>
/// Gives a window's own title bar the colours Windows gives the title bars it draws, so the bar
/// shows whether the window has the focus. The settings window and the APK Installer draw
/// their title bars themselves (their content extends into it), and WPF UI's title bar looks
/// the same whether its window is active or not.
///
/// The colours go into the window's resources under the keys <c>TitleBarStyle.xaml</c> reads:
/// its <c>WindowTitleBar</c> style switches between the active and the inactive ones on the
/// window's <see cref="Window.IsActive"/>. <see cref="TitleBarLook"/> holds the choice: the
/// backdrop and a dimmed title, the accent colour when "Show accent colour on title bars" is
/// on, or the high contrast theme's caption colours. They follow the theme, the accent colour
/// and the high contrast setting while the window is open.
/// </summary>
internal static class TitleBarActivation
{
    public const string ActiveBackground = "TitleBarActiveBackground";
    public const string ActiveForeground = "TitleBarActiveForeground";
    public const string InactiveBackground = "TitleBarInactiveBackground";
    public const string InactiveForeground = "TitleBarInactiveForeground";
    public const string InactiveIconOpacity = "TitleBarInactiveIconOpacity";

    private const int WmSysColorChange = 0x0015;
    private const int WmSettingChange = 0x001A;
    private const int WmThemeChanged = 0x031A;
    private const int WmDwmColorizationColorChanged = 0x0320;

    /// <summary>Fills <paramref name="window"/>'s title bar colours now and whenever Windows
    /// changes them. Call it after <c>InitializeComponent</c>.</summary>
    public static void Attach(Window window)
    {
        Apply(window);
        ThemeChangedEvent onTheme = (_, _) => Apply(window);
        ApplicationThemeManager.Changed += onTheme;
        window.Closed += (_, _) => ApplicationThemeManager.Changed -= onTheme;
        window.SourceInitialized += (_, _) =>
        {
            var source = HwndSource.FromHwnd(new WindowInteropHelper(window).Handle);
            source?.AddHook((IntPtr hwnd, int message, IntPtr wParam, IntPtr lParam, ref bool handled) =>
            {
                if (message is WmSysColorChange or WmSettingChange or WmThemeChanged or WmDwmColorizationColorChanged)
                {
                    // WPF reads the new system colours in its own handler of the same message,
                    // so the bar is coloured once that has run.
                    window.Dispatcher.InvokeAsync(() => Apply(window), DispatcherPriority.Background);
                }
                return IntPtr.Zero;
            });
        };
    }

    private static void Apply(Window window)
    {
        var look = TitleBarLook.For(ReadSystem());
        window.Resources[ActiveBackground] = Fill(look.ActiveFill);
        window.Resources[InactiveBackground] = Fill(look.InactiveFill);
        window.Resources[ActiveForeground] = Text(window, look.ActiveText, "TextFillColorPrimaryBrush");
        window.Resources[InactiveForeground] = Text(window, look.InactiveText, "TextFillColorDisabledBrush");
        window.Resources[InactiveIconOpacity] = look.InactiveIconOpacity;
    }

    /// <summary>A fill colour, or the backdrop: a clear brush, not none, so the bar still takes
    /// the pointer for dragging the window.</summary>
    private static Brush Fill(uint? argb) => argb is { } colour ? Frozen(colour) : Brushes.Transparent;

    /// <summary>A text colour, or the theme's brush of that name as the current theme has it.</summary>
    private static object Text(Window window, uint? argb, string themeKey) =>
        argb is { } colour ? Frozen(colour) : window.TryFindResource(themeKey) ?? SystemColors.ControlTextBrush;

    private static SolidColorBrush Frozen(uint argb)
    {
        var brush = new SolidColorBrush(Color.FromArgb((byte)(argb >> 24), (byte)(argb >> 16), (byte)(argb >> 8), (byte)argb));
        brush.Freeze();
        return brush;
    }

    private static uint Argb(Color c) => (uint)c.A << 24 | (uint)c.R << 16 | (uint)c.G << 8 | c.B;

    private static TitleBarSystem ReadSystem()
    {
        bool prevalence = false;
        uint? accent = null, inactive = null;
        try
        {
            using var dwm = Registry.CurrentUser.OpenSubKey(@"Software\Microsoft\Windows\DWM");
            prevalence = dwm?.GetValue("ColorPrevalence") is int on && on != 0;
            // AccentColor is the colour Windows 11 draws an active title bar in; Windows 10
            // without it uses the colorization colour, which is already 0xAARRGGBB.
            accent = dwm?.GetValue("AccentColor") is int abgr
                ? TitleBarLook.FromAbgr(unchecked((uint)abgr))
                : dwm?.GetValue("ColorizationColor") is int argb ? unchecked((uint)argb) : null;
            inactive = dwm?.GetValue("AccentColorInactive") is int inactiveAbgr
                ? TitleBarLook.FromAbgr(unchecked((uint)inactiveAbgr))
                : null;
        }
        catch (Exception e) when (e is System.Security.SecurityException or UnauthorizedAccessException or IOException)
        {
            // Unreadable settings leave the title bar on the backdrop.
        }
        return new TitleBarSystem(SystemParameters.HighContrast, prevalence, accent, inactive,
            Argb(SystemColors.ActiveCaptionColor), Argb(SystemColors.ActiveCaptionTextColor),
            Argb(SystemColors.InactiveCaptionColor), Argb(SystemColors.InactiveCaptionTextColor));
    }
}

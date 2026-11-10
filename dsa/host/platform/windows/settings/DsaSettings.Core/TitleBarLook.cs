namespace Dsa.Settings.Core;

/// <summary>
/// What Windows says about title bars, read by the window: "Show accent colour on title bars"
/// with the accent colours DWM draws them in, and the high contrast theme with its caption
/// colours. Colours are 0xAARRGGBB.
/// </summary>
public readonly record struct TitleBarSystem(
    bool HighContrast,
    bool AccentOnTitleBars,
    uint? Accent,
    uint? InactiveAccent,
    uint ActiveCaption,
    uint ActiveCaptionText,
    uint InactiveCaption,
    uint InactiveCaptionText);

/// <summary>
/// How a window's own title bar is drawn while the window is active and while it is not. A null
/// colour is the theme's: the window's backdrop (Mica on Windows 11) behind the bar, the theme's
/// primary text colour while active and its disabled text colour while inactive, as Windows 11
/// dims the title of a window that does not have the focus.
/// </summary>
public sealed record TitleBarLook(
    uint? ActiveFill,
    uint? ActiveText,
    uint? InactiveFill,
    uint? InactiveText,
    double InactiveIconOpacity)
{
    /// <summary>How much an inactive window's title bar icon shows, as its title text dims.</summary>
    public const double DimmedIconOpacity = 0.5;

    /// <summary>
    /// The look Windows gives a title bar it draws itself. High contrast draws the theme's
    /// caption colours, active and inactive, and dims nothing: the colours already differ.
    /// "Show accent colour on title bars" fills an active window's bar with the accent colour
    /// and its title in black or white, whichever reads on it; an inactive window's bar takes
    /// the inactive accent colour when one is set, and the backdrop otherwise. Without either,
    /// the bar is the backdrop and only the title dims when the window loses the focus.
    /// </summary>
    public static TitleBarLook For(TitleBarSystem system)
    {
        if (system.HighContrast)
        {
            return new TitleBarLook(system.ActiveCaption, system.ActiveCaptionText,
                system.InactiveCaption, system.InactiveCaptionText, 1);
        }
        if (system.AccentOnTitleBars && system.Accent is { } accent)
        {
            var active = Opaque(accent);
            if (system.InactiveAccent is { } inactiveAccent)
            {
                var inactive = Opaque(inactiveAccent);
                return new TitleBarLook(active, TextOn(active), inactive, Dimmed(TextOn(inactive)), DimmedIconOpacity);
            }
            return new TitleBarLook(active, TextOn(active), null, null, DimmedIconOpacity);
        }
        return new TitleBarLook(null, null, null, null, DimmedIconOpacity);
    }

    /// <summary>Black on a light colour, white on a dark one: the rule Windows documents for
    /// text on the accent colour, which weighs green most, then red, then blue.</summary>
    public static uint TextOn(uint argb)
    {
        var r = (argb >> 16) & 0xFF;
        var g = (argb >> 8) & 0xFF;
        var b = argb & 0xFF;
        return 5 * g + 2 * r + b > 8 * 128 ? 0xFF000000u : 0xFFFFFFFFu;
    }

    /// <summary>DWM keeps its accent colours as 0xAABBGGRR; this turns one into 0xAARRGGBB.</summary>
    public static uint FromAbgr(uint abgr) =>
        (abgr & 0xFF000000u) | ((abgr & 0xFF) << 16) | (abgr & 0xFF00) | ((abgr >> 16) & 0xFF);

    private static uint Opaque(uint argb) => argb | 0xFF000000u;

    /// <summary>A title colour at the alpha of the theme's disabled text (about 36 %).</summary>
    private static uint Dimmed(uint argb) => (argb & 0x00FFFFFFu) | 0x5C000000u;
}

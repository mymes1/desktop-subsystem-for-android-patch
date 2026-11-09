namespace Dsa.Settings.Core;

/// <summary>A rectangle in a monitor's physical pixels, as Windows reports a work area.</summary>
public readonly record struct PixelRect(int Left, int Top, int Width, int Height)
{
    public int Right => Left + Width;
    public int Bottom => Top + Height;
}

/// <summary>
/// Where a window opens: at its preferred size when that fits the work area of the monitor it
/// opens on, smaller when it does not, and always inside that work area.
///
/// Centring a window taller than the work area puts its title bar above the top of the screen,
/// where it cannot be grabbed to move the window. A 1040 x 760 window does that on a laptop at a
/// high scale: 2880 x 1800 at 250 % is 1152 x 720 DIPs before the taskbar takes its share.
///
/// Sizes come in DIPs (1/96 inch) and are scaled by the monitor's own DPI, because with
/// per-monitor DPI every monitor has its own scale and the work area is in physical pixels.
/// </summary>
public static class WindowFit
{
    /// <summary>The gap kept between a shrunk window and the work area's edges, in DIPs.</summary>
    public const double Margin = 16;

    /// <summary>
    /// The window's rectangle in physical pixels. A preferred size that fits inside the work
    /// area less <see cref="Margin"/> on each side is kept; a larger one shrinks to that, but
    /// never below the minimum. The window is centred in the work area; one that is still larger
    /// than the work area (its minimum does not fit) starts at the work area's top left edge, so
    /// its title bar stays on the screen.
    /// </summary>
    public static PixelRect Place(PixelRect workArea, double dpi,
        double preferredWidth, double preferredHeight, double minWidth, double minHeight)
    {
        var scale = dpi > 0 ? dpi / 96.0 : 1.0;
        var margin = (int)Math.Round(Margin * scale);
        var width = Fit(preferredWidth, minWidth, workArea.Width, margin, scale);
        var height = Fit(preferredHeight, minHeight, workArea.Height, margin, scale);
        return new PixelRect(Start(workArea.Left, workArea.Width, width),
            Start(workArea.Top, workArea.Height, height), width, height);
    }

    private static int Fit(double preferred, double minimum, int available, int margin, double scale)
    {
        var wanted = (int)Math.Round(preferred * scale);
        var least = (int)Math.Ceiling(minimum * scale);
        var room = available - 2 * margin;
        return Math.Max(least, Math.Min(wanted, room));
    }

    private static int Start(int origin, int available, int size) =>
        size >= available ? origin : origin + (available - size) / 2;
}

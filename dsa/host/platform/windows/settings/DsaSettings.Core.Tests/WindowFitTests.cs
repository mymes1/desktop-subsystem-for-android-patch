using Dsa.Settings.Core;
using Xunit;

namespace Dsa.Settings.Core.Tests;

/// <summary>The settings window and the APK Installer open inside the work area of their
/// monitor: at their preferred size when it fits, smaller when it does not, and never with the
/// title bar above the top of the screen. Work areas are in physical pixels, sizes in DIPs.</summary>
public class WindowFitTests
{
    private static bool Inside(PixelRect window, PixelRect work) =>
        window.Left >= work.Left && window.Top >= work.Top
        && window.Right <= work.Right && window.Bottom <= work.Bottom;

    [Fact]
    public void Fr17_a_window_that_fits_keeps_its_preferred_size_and_is_centred()
    {
        // 1920 x 1080 at 100 %, a 48 px taskbar at the bottom.
        var work = new PixelRect(0, 0, 1920, 1032);
        var placed = WindowFit.Place(work, 96, 1040, 760, 640, 360);
        Assert.Equal(new PixelRect(440, 136, 1040, 760), placed);
    }

    [Fact]
    public void Fr17_the_settings_window_shrinks_to_a_high_scale_laptop_screen()
    {
        // 2880 x 1800 at 250 % is 1152 x 720 DIPs; the taskbar takes 48 DIPs (120 px).
        var work = new PixelRect(0, 0, 2880, 1680);
        var placed = WindowFit.Place(work, 240, 1040, 760, 640, 360);
        // The width fits and stays 1040 DIPs; the height is the work area less 16 DIPs a side.
        Assert.Equal(2600, placed.Width);
        Assert.Equal(1680 - 2 * 40, placed.Height);
        Assert.True(Inside(placed, work));
        Assert.Equal(40, placed.Top);
    }

    [Fact]
    public void Fr17_the_size_scales_by_the_dpi_of_the_monitor_it_opens_on()
    {
        // A second monitor left of the primary, at 150 %: its work area has negative coordinates.
        var work = new PixelRect(-2560, 0, 2560, 1380);
        var placed = WindowFit.Place(work, 144, 600, 580, 480, 420);
        Assert.Equal(900, placed.Width);
        Assert.Equal(870, placed.Height);
        Assert.True(Inside(placed, work));
    }

    [Fact]
    public void Fr17_a_window_never_shrinks_below_its_minimum()
    {
        var work = new PixelRect(0, 0, 660, 300);
        var placed = WindowFit.Place(work, 96, 1040, 760, 640, 360);
        Assert.Equal(640, placed.Width);
        Assert.Equal(360, placed.Height);
    }

    [Fact]
    public void Fr17_a_window_larger_than_the_work_area_keeps_its_title_bar_on_the_screen()
    {
        // The work area starts below a taskbar docked at the top.
        var work = new PixelRect(0, 48, 600, 300);
        var placed = WindowFit.Place(work, 96, 1040, 760, 640, 360);
        Assert.Equal(0, placed.Left);
        Assert.Equal(48, placed.Top);
    }

    [Theory]
    [InlineData(96)]
    [InlineData(120)]
    [InlineData(144)]
    [InlineData(192)]
    [InlineData(240)]
    public void Fr17_the_window_is_inside_the_work_area_at_every_common_scale(double dpi)
    {
        // A 1366 x 768 laptop and a 3840 x 2160 monitor, each less a 48 DIP taskbar.
        foreach (var (width, height) in new[] { (1366, 768), (3840, 2160), (2880, 1800) })
        {
            var taskbar = (int)Math.Round(48 * dpi / 96);
            var work = new PixelRect(0, 0, width, height - taskbar);
            var minWidth = 640 * dpi / 96;
            var minHeight = 360 * dpi / 96;
            if (minWidth > work.Width || minHeight > work.Height)
            {
                continue;
            }
            var placed = WindowFit.Place(work, dpi, 1040, 760, 640, 360);
            Assert.True(Inside(placed, work), $"{width} x {height} at {dpi} dpi: {placed}");
        }
    }
}

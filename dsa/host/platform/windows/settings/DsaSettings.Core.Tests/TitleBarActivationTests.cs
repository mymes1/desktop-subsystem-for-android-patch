using System.Xml.Linq;
using Dsa.Settings.Core;
using Xunit;

namespace Dsa.Settings.Core.Tests;

/// <summary>The settings window and the APK Installer draw their own title bars. These show
/// whether the window has the focus as the title bars Windows draws do: full strength on the
/// backdrop or the accent colour while active, dimmed on the backdrop while not, and the high
/// contrast theme's caption colours.</summary>
public class TitleBarActivationTests
{
    private static readonly XNamespace Wpf = "http://schemas.microsoft.com/winfx/2006/xaml/presentation";
    private static readonly XNamespace X = "http://schemas.microsoft.com/winfx/2006/xaml";
    private static readonly XNamespace Ui = "http://schemas.lepo.co/wpfui/2022/xaml";

    private static string FilePath(string app, string file)
    {
        // The tests run from dsa/target/settings/..., outside the source tree, so each folder
        // above is tried both as the settings folder and as the repository's dsa folder.
        string[] relative = { Path.Combine(app, file),
            Path.Combine("host", "platform", "windows", "settings", app, file) };
        string? found = null;
        for (var dir = new DirectoryInfo(AppContext.BaseDirectory); dir is not null && found is null; dir = dir.Parent)
        {
            found = relative.Select(r => Path.Combine(dir.FullName, r)).FirstOrDefault(File.Exists);
        }
        Assert.True(found is not null, file + " was not found above " + AppContext.BaseDirectory);
        return found!;
    }

    private static XDocument Xaml(string app, string file) => XDocument.Load(FilePath(app, file));

    private static XElement KeyedStyle(string key) => Xaml("DsaSettings", "TitleBarStyle.xaml")
        .Descendants(Wpf + "Style").Single(s => (string?)s.Attribute(X + "Key") == key);

    private static Dictionary<string, string?> Setters(XElement parent) => parent.Elements(Wpf + "Setter")
        .ToDictionary(s => (string)s.Attribute("Property")!, s => (string?)s.Attribute("Value"));

    /// <summary>The style's trigger on its window losing the focus.</summary>
    private static XElement InactiveTrigger(XElement style)
    {
        var trigger = style.Descendants(Wpf + "DataTrigger").Single();
        var binding = (string?)trigger.Attribute("Binding");
        Assert.Equal("{Binding IsActive, RelativeSource={RelativeSource AncestorType={x:Type Window}}}", binding);
        Assert.Equal("False", (string?)trigger.Attribute("Value"));
        return trigger;
    }

    [Theory]
    [InlineData("DsaApkInstaller", "InstallerWindow")]
    [InlineData("DsaSettings", "MainWindow")]
    public void Fr17_the_title_bar_follows_the_windows_activation(string app, string window)
    {
        // WPF UI's title bar looked the same whether its window had the focus or not, so the
        // installer's stayed grey and full strength behind other windows.
        var bar = Xaml(app, window + ".xaml").Descendants(Ui + "TitleBar").Single();
        Assert.Equal("{StaticResource WindowTitleBar}", (string?)bar.Attribute("Style"));

        var style = KeyedStyle("WindowTitleBar");
        Assert.Equal("ui:TitleBar", (string?)style.Attribute("TargetType"));
        // It colours the library's title bar and keeps its template.
        Assert.Equal("{StaticResource {x:Type ui:TitleBar}}", (string?)style.Attribute("BasedOn"));
        Assert.Equal(new Dictionary<string, string?>
        {
            ["Background"] = "{DynamicResource TitleBarActiveBackground}",
            ["Foreground"] = "{DynamicResource TitleBarActiveForeground}",
            ["ButtonsForeground"] = "{DynamicResource TitleBarActiveForeground}",
        }, Setters(style));
        Assert.Equal(new Dictionary<string, string?>
        {
            ["Background"] = "{DynamicResource TitleBarInactiveBackground}",
            ["Foreground"] = "{DynamicResource TitleBarInactiveForeground}",
            ["ButtonsForeground"] = "{DynamicResource TitleBarInactiveForeground}",
        }, Setters(InactiveTrigger(style)));

        // The app loads the style, and the window fills the colours it names.
        var merged = Xaml(app, "App.xaml").Descendants(Wpf + "ResourceDictionary")
            .Select(d => (string?)d.Attribute("Source"));
        Assert.Contains("/TitleBarStyle.xaml", merged);
        Assert.Contains("TitleBarActivation.Attach(this);", File.ReadAllText(FilePath(app, window + ".xaml.cs")));
        var colours = File.ReadAllText(FilePath("DsaSettings", "TitleBarActivation.cs"));
        foreach (var key in new[] { "TitleBarActiveBackground", "TitleBarActiveForeground", "TitleBarInactiveBackground",
            "TitleBarInactiveForeground", "TitleBarInactiveIconOpacity" })
        {
            Assert.Contains("\"" + key + "\"", colours);
        }
    }

    [Fact]
    public void Fr17_the_installer_builds_the_settings_apps_title_bar()
    {
        var project = Xaml("DsaApkInstaller", "DsaApkInstaller.csproj");
        Assert.Contains(project.Descendants("Compile"),
            e => (string?)e.Attribute("Include") == @"..\DsaSettings\TitleBarActivation.cs");
        Assert.Contains(project.Descendants("Page"),
            e => (string?)e.Attribute("Include") == @"..\DsaSettings\TitleBarStyle.xaml"
                && (string?)e.Attribute("Link") == "TitleBarStyle.xaml");
    }

    [Fact]
    public void Fr17_the_settings_title_bar_icon_dims_with_its_title()
    {
        var icon = Xaml("DsaSettings", "MainWindow.xaml").Descendants(Ui + "ImageIcon").Single();
        Assert.Equal("{StaticResource WindowTitleBarIcon}", (string?)icon.Attribute("Style"));
        Assert.Equal(new Dictionary<string, string?> { ["Opacity"] = "{DynamicResource TitleBarInactiveIconOpacity}" },
            Setters(InactiveTrigger(KeyedStyle("WindowTitleBarIcon"))));
    }

    private static TitleBarSystem Reported(bool highContrast = false, bool accentOn = false, uint? accent = null,
        uint? inactiveAccent = null) =>
        new(highContrast, accentOn, accent, inactiveAccent, 0xFF1AEBFF, 0xFF000000, 0xFF000000, 0xFF3FF23F);

    [Fact]
    public void Fr17_by_default_the_bar_is_the_backdrop_and_only_the_title_dims()
    {
        // The accent colour is set but not shown on title bars.
        var look = TitleBarLook.For(Reported(accent: 0xFF0078D4));
        Assert.Null(look.ActiveFill);
        Assert.Null(look.ActiveText);
        Assert.Null(look.InactiveFill);
        Assert.Null(look.InactiveText);
        Assert.Equal(TitleBarLook.DimmedIconOpacity, look.InactiveIconOpacity);
        Assert.True(look.InactiveIconOpacity < 1);
    }

    [Theory]
    // Windows' default blue and a dark green take white; a yellow and a light pink take black.
    [InlineData(0xFF0078D4u, 0xFFFFFFFFu)]
    [InlineData(0xFF107C10u, 0xFFFFFFFFu)]
    [InlineData(0xFFFFB900u, 0xFF000000u)]
    [InlineData(0xFFF7C6E0u, 0xFF000000u)]
    public void Fr17_accent_on_title_bars_fills_the_active_bar_with_readable_text(uint accent, uint text)
    {
        var look = TitleBarLook.For(Reported(accentOn: true, accent: accent));
        Assert.Equal(accent, look.ActiveFill);
        Assert.Equal(text, look.ActiveText);
        // Inactive, the bar goes back to the backdrop with the dimmed title.
        Assert.Null(look.InactiveFill);
        Assert.Null(look.InactiveText);
    }

    [Fact]
    public void Fr17_an_inactive_accent_colour_fills_the_inactive_bar_with_a_dimmed_title()
    {
        var look = TitleBarLook.For(Reported(accentOn: true, accent: 0xFF0078D4, inactiveAccent: 0xFF2B2B2B));
        Assert.Equal(0xFF2B2B2Bu, look.InactiveFill);
        Assert.Equal(0x5CFFFFFFu, look.InactiveText);
    }

    [Fact]
    public void Fr17_high_contrast_takes_the_themes_caption_colours_and_dims_nothing()
    {
        var look = TitleBarLook.For(Reported(highContrast: true, accentOn: true, accent: 0xFF0078D4));
        Assert.Equal(0xFF1AEBFFu, look.ActiveFill);
        Assert.Equal(0xFF000000u, look.ActiveText);
        Assert.Equal(0xFF000000u, look.InactiveFill);
        Assert.Equal(0xFF3FF23Fu, look.InactiveText);
        Assert.Equal(1, look.InactiveIconOpacity);
    }

    [Fact]
    public void Fr17_dwm_accent_colours_are_read_as_abgr()
    {
        // Windows' default blue, #0078D4, as DWM's AccentColor stores it.
        Assert.Equal(0xFF0078D4u, TitleBarLook.FromAbgr(0xFFD47800u));
    }
}

using System.Xml.Linq;
using Xunit;

namespace Dsa.Settings.Core.Tests;

/// <summary>The APK Installer is laid out as App Installer lays out an app package. These hold
/// the metrics that make it look so: the Windows type ramp, the spacing, Mica, the icon tile at
/// the top right and the main button at the bottom right.</summary>
public class ApkInstallerStyleTests
{
    private static readonly XNamespace Wpf = "http://schemas.microsoft.com/winfx/2006/xaml/presentation";
    private static readonly XNamespace X = "http://schemas.microsoft.com/winfx/2006/xaml";
    private static readonly XNamespace Ui = "http://schemas.lepo.co/wpfui/2022/xaml";

    private static XDocument Xaml(string file)
    {
        // The tests run from dsa/target/settings/..., outside the source tree, so each folder
        // above is tried both as the settings folder and as the repository's dsa folder.
        string[] relative = { Path.Combine("DsaApkInstaller", file),
            Path.Combine("host", "platform", "windows", "settings", "DsaApkInstaller", file) };
        string? found = null;
        for (var dir = new DirectoryInfo(AppContext.BaseDirectory); dir is not null && found is null; dir = dir.Parent)
        {
            found = relative.Select(r => Path.Combine(dir.FullName, r)).FirstOrDefault(File.Exists);
        }
        Assert.True(found is not null, file + " was not found above " + AppContext.BaseDirectory);
        return XDocument.Load(found!);
    }

    private static XDocument AppXaml() => Xaml("App.xaml");

    private static XDocument Window() => Xaml("InstallerWindow.xaml");

    private static XElement Named(XDocument doc, string name) =>
        doc.Descendants().Single(e => (string?)e.Attribute(X + "Name") == name);

    private static XElement KeyedStyle(string key) => AppXaml().Descendants(Wpf + "Style")
        .Single(s => (string?)s.Attribute(X + "Key") == key);

    private static string? Setter(XElement style, string property) => style.Elements(Wpf + "Setter")
        .FirstOrDefault(s => (string?)s.Attribute("Property") == property)?.Attribute("Value")?.Value;

    private static string? Resource(string key) => AppXaml().Root!.Descendants()
        .FirstOrDefault(e => (string?)e.Attribute(X + "Key") == key && !e.HasElements)?.Value;

    [Theory]
    // The WinUI type ramp: Title for the app's name, Body Strong, Body and Caption.
    [InlineData("AppNameText", "28", "36", "Normal", "{StaticResource StrongDisplayFont}")]
    [InlineData("SectionHeader", "14", "20", "Normal", "{StaticResource StrongTextFont}")]
    [InlineData("DetailLine", "14", "20", "Normal", "{StaticResource TextFont}")]
    [InlineData("BodyText", "14", "20", "Normal", "{StaticResource TextFont}")]
    [InlineData("CaptionText", "12", "16", "Normal", "{StaticResource TextFont}")]
    public void Fr11_installer_text_follows_the_windows_type_ramp(string key, string size, string line, string weight, string font)
    {
        var style = KeyedStyle(key);
        Assert.Equal(size, Setter(style, "FontSize"));
        Assert.Equal(line, Setter(style, "LineHeight"));
        Assert.Equal(weight, Setter(style, "FontWeight"));
        Assert.Equal(font, Setter(style, "FontFamily"));
    }

    [Fact]
    public void Fr11_installer_fonts_are_segoe_ui_variable_with_korean_fallback()
    {
        Assert.Equal("Segoe UI Variable Display, Segoe UI, Malgun Gothic", Resource("DisplayFont"));
        Assert.Equal("Segoe UI Variable Text, Segoe UI, Malgun Gothic", Resource("TextFont"));
    }

    [Fact]
    public void Fr11_installer_keeps_app_installers_spacing()
    {
        Assert.Equal("32,12,32,0", Resource("ContentMargin"));
        Assert.Equal("32,20,32,24", Resource("FooterPadding"));
        Assert.Equal("88", Resource("AppIconSize"));
        Assert.Equal("120", Resource("PrimaryButtonMinWidth"));
        Assert.Equal("0,20,0,8", Setter(KeyedStyle("SectionHeader"), "Margin"));
    }

    [Fact]
    public void Fr11_installer_window_has_mica_and_its_own_title_bar()
    {
        var root = Window().Root!;
        Assert.Equal(Ui + "FluentWindow", root.Name);
        Assert.Equal("Mica", (string?)root.Attribute("WindowBackdropType"));
        Assert.Equal("True", (string?)root.Attribute("ExtendsContentIntoTitleBar"));
        Assert.Equal("Round", (string?)root.Attribute("WindowCornerPreference"));
        // The installer tests find the window by this id.
        Assert.Equal("ApkInstallWindow", (string?)root.Attribute(XName.Get("AutomationProperties.AutomationId")));
        var bar = Window().Descendants(Ui + "TitleBar").Single();
        Assert.Equal("False", (string?)bar.Attribute("ShowMaximize"));
        var icon = bar.Descendants(Ui + "ImageIcon").Single();
        Assert.Equal("pack://application:,,,/Assets/apk-installer.png", (string?)icon.Attribute("Source"));
    }

    [Fact]
    public void Fr11_the_app_name_is_large_at_the_top_left_and_the_icon_at_the_top_right()
    {
        var doc = Window();
        var header = Named(doc, "Header");
        var left = header.Elements(Wpf + "StackPanel").Single();
        Assert.Equal("0", (string?)left.Attribute("Grid.Column"));
        var lines = left.Elements(Wpf + "TextBlock").Select(t => (string?)t.Attribute(X + "Name")).ToList();
        Assert.Equal(new[] { "HeroTitle", "HeroPackage", "PublisherText", "VersionChip" }, lines);
        Assert.Equal("{StaticResource AppNameText}", (string?)Named(doc, "HeroTitle").Attribute("Style"));
        Assert.Equal("{StaticResource DetailLine}", (string?)Named(doc, "PublisherText").Attribute("Style"));
        Assert.Equal("{StaticResource DetailLine}", (string?)Named(doc, "VersionChip").Attribute("Style"));
        var icon = Named(doc, "AppIcon");
        Assert.Equal("1", (string?)icon.Attribute("Grid.Column"));
        Assert.Equal("Top", (string?)icon.Attribute("VerticalAlignment"));
        Assert.Equal("{StaticResource AppIconSize}", (string?)icon.Attribute("Width"));
        Assert.Equal("{StaticResource AppIconSize}", (string?)icon.Attribute("Height"));
    }

    [Fact]
    public void Fr11_capabilities_come_before_the_details()
    {
        var doc = Window();
        var names = Named(doc, "DetailsPanel").Descendants()
            .Select(e => (string?)e.Attribute(X + "Name")).Where(n => n is not null).ToList();
        Assert.True(names.IndexOf("PermissionsHeader") < names.IndexOf("PermissionList"));
        Assert.True(names.IndexOf("PermissionList") < names.IndexOf("DetailsHeader"));
        Assert.True(names.IndexOf("DetailsHeader") < names.IndexOf("FactList"));
        Assert.Equal("{StaticResource SectionHeader}", (string?)Named(doc, "PermissionsHeader").Attribute("Style"));
    }

    [Fact]
    public void Fr11_launch_when_ready_sits_left_and_the_main_button_right_in_the_footer()
    {
        var doc = Window();
        var check = Named(doc, "LaunchWhenReady");
        Assert.Equal(Wpf + "CheckBox", check.Name);
        Assert.Equal("0", (string?)check.Attribute("Grid.Column"));
        Assert.Equal("True", (string?)check.Attribute("IsChecked"));
        // The progress takes the checkbox's place while the app installs.
        Assert.Equal("0", (string?)Named(doc, "ProgressPanel").Attribute("Grid.Column"));
        var buttons = Named(doc, "Buttons");
        Assert.Equal("1", (string?)buttons.Attribute("Grid.Column"));
        Assert.Equal("Right", (string?)buttons.Attribute("HorizontalAlignment"));
        // The main buttons come last, at the right edge, in the accent colour.
        var order = buttons.Elements().Select(b => (string?)b.Attribute(X + "Name")).ToList();
        Assert.Equal(new[] { "CancelButton", "CloseButton", "OpenButton", "InstallButton" }, order);
        foreach (var name in new[] { "InstallButton", "OpenButton" })
        {
            Assert.Equal("Primary", (string?)Named(doc, name).Attribute("Appearance"));
        }
        foreach (var button in buttons.Elements())
        {
            Assert.Equal("{StaticResource PrimaryButtonMinWidth}", (string?)button.Attribute("MinWidth"));
        }
        Assert.Equal("{StaticResource FooterPadding}", (string?)Named(doc, "Footer").Attribute("Padding"));
    }
}

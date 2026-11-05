using System.Xml.Linq;

namespace Dsa.Settings.Core.Tests;

/// <summary>The settings cards are drawn by templates in App.xaml. A card template that loses
/// its fill or its border leaves the header and the control floating on the page, so the
/// styles must set both from the theme's card brushes and the templates must draw them.</summary>
public class CardStyleTests
{
    private static readonly XNamespace Wpf = "http://schemas.microsoft.com/winfx/2006/xaml/presentation";

    private static XDocument AppXaml()
    {
        var dir = new DirectoryInfo(AppContext.BaseDirectory);
        while (dir is not null && !File.Exists(Path.Combine(dir.FullName, "DsaSettings", "App.xaml")))
        {
            dir = dir.Parent;
        }
        Assert.True(dir is not null, "App.xaml was not found above " + AppContext.BaseDirectory);
        return XDocument.Load(Path.Combine(dir!.FullName, "DsaSettings", "App.xaml"));
    }

    [Theory]
    [InlineData("ui:CardControl")]
    [InlineData("ui:CardAction")]
    public void Fr17_a_settings_card_has_the_card_fill_and_border(string type)
    {
        var style = AppXaml().Descendants(Wpf + "Style")
            .SingleOrDefault(s => (string?)s.Attribute("TargetType") == type);
        Assert.True(style is not null, $"App.xaml has no style for {type}");

        string? Setter(string property) => style!.Elements(Wpf + "Setter")
            .FirstOrDefault(s => (string?)s.Attribute("Property") == property)?.Attribute("Value")?.Value;

        Assert.Equal("{DynamicResource CardBackgroundFillColorDefaultBrush}", Setter("Background"));
        Assert.Equal("{DynamicResource CardStrokeColorDefaultBrush}", Setter("BorderBrush"));
        Assert.Equal("1", Setter("BorderThickness"));

        var border = style!.Descendants(Wpf + "ControlTemplate").Single()
            .Descendants(Wpf + "Border").First();
        Assert.Equal("{TemplateBinding Background}", (string?)border.Attribute("Background"));
        Assert.Equal("{TemplateBinding BorderBrush}", (string?)border.Attribute("BorderBrush"));
        Assert.Equal("{TemplateBinding BorderThickness}", (string?)border.Attribute("BorderThickness"));
        Assert.Equal("4", (string?)border.Attribute("CornerRadius"));
    }
}

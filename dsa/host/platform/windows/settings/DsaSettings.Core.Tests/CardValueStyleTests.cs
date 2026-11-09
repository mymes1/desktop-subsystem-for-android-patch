using System.Xml.Linq;
using Xunit;

namespace Dsa.Settings.Core.Tests;

/// <summary>A value a settings card shows on its right (a size, a version, a build) sits against
/// the card's right edge, as in the Windows Settings app. The card keeps at least 120 px for its
/// control, so a text left at its default alignment starts 120 px from the edge: a short value
/// hangs in the middle of that box while a longer one reaches the edge, and the values in a list
/// of cards stop lining up. One style, CardValue in App.xaml, places every such text.</summary>
public class CardValueStyleTests
{
    private static readonly XNamespace Wpf = "http://schemas.microsoft.com/winfx/2006/xaml/presentation";

    private static readonly XNamespace X = "http://schemas.microsoft.com/winfx/2006/xaml";

    private static readonly XNamespace Ui = "http://schemas.lepo.co/wpfui/2022/xaml";

    private static string SettingsApp()
    {
        // The tests run from dsa/target/settings/..., outside the source tree, so each folder
        // above is tried both as the settings folder and as the repository's dsa folder.
        string[] relative = { "DsaSettings", Path.Combine("host", "platform", "windows", "settings", "DsaSettings") };
        for (var dir = new DirectoryInfo(AppContext.BaseDirectory); dir is not null; dir = dir.Parent)
        {
            var found = relative.Select(r => Path.Combine(dir.FullName, r))
                .FirstOrDefault(d => File.Exists(Path.Combine(d, "App.xaml")) && File.Exists(Path.Combine(d, "DsaSettings.csproj")));
            if (found is not null)
            {
                return found;
            }
        }
        Assert.Fail("the settings app's source was not found above " + AppContext.BaseDirectory);
        return "";
    }

    private static XDocument AppXaml() => XDocument.Load(Path.Combine(SettingsApp(), "App.xaml"));

    private static string? Setter(XElement style, string property) => style.Elements(Wpf + "Setter")
        .FirstOrDefault(s => (string?)s.Attribute("Property") == property)?.Attribute("Value")?.Value;

    private static string? StaticKey(string? reference)
    {
        const string prefix = "{StaticResource ";
        return reference is not null && reference.StartsWith(prefix, StringComparison.Ordinal) && reference.EndsWith('}')
            ? reference[prefix.Length..^1].Trim()
            : null;
    }

    /// <summary>The keys of CardValue and of every style based on it, directly or through another.</summary>
    private static HashSet<string> CardValueStyles()
    {
        var styles = AppXaml().Descendants(Wpf + "Style")
            .Where(s => s.Attribute(X + "Key") is not null)
            .ToDictionary(s => (string)s.Attribute(X + "Key")!, s => StaticKey((string?)s.Attribute("BasedOn")));
        var keys = new HashSet<string> { "CardValue" };
        bool grew;
        do
        {
            grew = false;
            foreach (var (key, basedOn) in styles)
            {
                if (basedOn is not null && keys.Contains(basedOn) && keys.Add(key))
                {
                    grew = true;
                }
            }
        }
        while (grew);
        return keys;
    }

    /// <summary>Every TextBlock in a settings card's content (not its header), in every page and
    /// window of the settings app.</summary>
    private static List<(string file, XElement text)> CardContentTexts()
    {
        return Directory.GetFiles(SettingsApp(), "*.xaml", SearchOption.AllDirectories)
            .Where(file => !file.Contains(Path.DirectorySeparatorChar + "obj" + Path.DirectorySeparatorChar)
                && !file.Contains(Path.DirectorySeparatorChar + "bin" + Path.DirectorySeparatorChar))
            .SelectMany(file => XDocument.Load(file).Descendants(Ui + "CardControl")
                .SelectMany(card => card.Elements().Where(e => !e.Name.LocalName.Contains('.')))
                .SelectMany(content => content.DescendantsAndSelf(Wpf + "TextBlock"))
                .Select(text => (Path.GetFileName(file), text)))
            .ToList();
    }

    [Fact]
    public void Fr17_a_card_value_sits_against_the_card_right_edge()
    {
        var style = AppXaml().Descendants(Wpf + "Style")
            .SingleOrDefault(s => (string?)s.Attribute(X + "Key") == "CardValue");
        Assert.True(style is not null, "App.xaml has no CardValue style");
        Assert.Equal("TextBlock", (string?)style!.Attribute("TargetType"));
        // Right in its box and right on every line, so a short value and a wrapped one both end
        // at the card's edge, and never wider than one width on every page.
        Assert.Equal("Right", Setter(style, "HorizontalAlignment"));
        Assert.Equal("Right", Setter(style, "TextAlignment"));
        Assert.Equal("Wrap", Setter(style, "TextWrapping"));
        Assert.Equal("320", Setter(style, "MaxWidth"));
        Assert.Equal("14", Setter(style, "FontSize"));
        Assert.Equal("20", Setter(style, "LineHeight"));
        Assert.Equal("Normal", Setter(style, "FontWeight"));
        // No colour of its own, so a disabled card greys its value as it greys its header.
        Assert.Null(Setter(style, "Foreground"));
    }

    [Fact]
    public void Fr17_a_style_based_on_card_value_keeps_its_placement()
    {
        var keys = CardValueStyles();
        Assert.Contains("CardValueCaption", keys);
        Assert.Contains("CardValueSubtitle", keys);
        foreach (var style in AppXaml().Descendants(Wpf + "Style")
            .Where(s => keys.Contains((string?)s.Attribute(X + "Key") ?? "") && (string?)s.Attribute(X + "Key") != "CardValue"))
        {
            foreach (var property in new[] { "HorizontalAlignment", "TextAlignment", "TextWrapping", "MaxWidth" })
            {
                Assert.True(Setter(style, property) is null,
                    $"App.xaml style {(string?)style.Attribute(X + "Key")} sets {property} again, so its values no longer line up with the others");
            }
        }
    }

    [Fact]
    public void Fr17_every_text_in_a_card_content_uses_the_card_value_style()
    {
        var keys = CardValueStyles();
        var texts = CardContentTexts();
        // The values the Memory and Updates pages showed out of line.
        Assert.Contains(texts, t => t.file == "UpdatesPage.xaml" && ((string?)t.text.Attribute("Text"))?.Contains("SecurityPatch") == true);
        Assert.Contains(texts, t => t.file == "MemoryPage.xaml" && (string?)t.text.Attribute("Text") == "{Binding Size}");

        var offenders = texts
            .Where(t => !keys.Contains(StaticKey((string?)t.text.Attribute("Style")) ?? ""))
            .Select(t => $"{t.file}: {(string?)t.text.Attribute("Text") ?? (string?)t.text.Attribute(X + "Name") ?? "TextBlock"}")
            .ToList();
        Assert.True(offenders.Count == 0,
            "these texts in a card's content do not use the CardValue style, so they do not sit against the card's right edge: "
            + string.Join(", ", offenders));
    }

    [Fact]
    public void Fr17_no_card_value_overrides_its_placement()
    {
        string[] placement = { "HorizontalAlignment", "TextAlignment", "TextWrapping", "MaxWidth" };
        var offenders = CardContentTexts()
            .SelectMany(t => placement.Where(p => t.text.Attribute(p) is not null)
                .Select(p => $"{t.file}: {(string?)t.text.Attribute("Text") ?? (string?)t.text.Attribute(X + "Name") ?? "TextBlock"} sets {p}"))
            .ToList();
        Assert.True(offenders.Count == 0,
            "these card values set their own placement, so they no longer line up with the others: "
            + string.Join(", ", offenders));
    }
}

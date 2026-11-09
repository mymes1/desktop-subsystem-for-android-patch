using System.Globalization;
using System.Xml.Linq;
using Xunit;

namespace Dsa.Settings.Core.Tests;

/// <summary>Everything in a page lines up with the settings cards, as in the Windows Settings
/// app. A card's switch ends at the card's right edge like its buttons and values: left at its
/// default alignment, a switch filled the card's 120 px control box from the left and ended about
/// 50 px before the edge on the Display, Updates, Advanced and Network pages. A notice bar between
/// two cards puts its icon in the cards' icon column and its title in their title column: the
/// library's bar started both further left, so the General page's "Last stopped" and "Update"
/// bars did not line up with the cards around them.</summary>
public class CardAlignmentTests
{
    private static readonly XNamespace Wpf = "http://schemas.microsoft.com/winfx/2006/xaml/presentation";

    private static readonly XNamespace X = "http://schemas.microsoft.com/winfx/2006/xaml";

    private static readonly XNamespace Ui = "http://schemas.lepo.co/wpfui/2022/xaml";

    private const string CardControls = "{StaticResource CardControls}";

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

    private static IEnumerable<(string file, XDocument doc)> Pages() =>
        Directory.GetFiles(SettingsApp(), "*.xaml", SearchOption.AllDirectories)
            .Where(file => !file.Contains(Path.DirectorySeparatorChar + "obj" + Path.DirectorySeparatorChar)
                && !file.Contains(Path.DirectorySeparatorChar + "bin" + Path.DirectorySeparatorChar)
                && Path.GetFileName(file) != "App.xaml")
            .Select(file => (Path.GetFileName(file), XDocument.Load(file)));

    private static XElement TypedStyle(string type) => AppXaml().Descendants(Wpf + "Style")
        .Single(s => (string?)s.Attribute("TargetType") == type && s.Attribute(X + "Key") is null);

    private static XElement KeyedStyle(string key) => AppXaml().Descendants(Wpf + "Style")
        .Single(s => (string?)s.Attribute(X + "Key") == key);

    private static string? Setter(XElement style, string property) => style.Elements(Wpf + "Setter")
        .FirstOrDefault(s => (string?)s.Attribute("Property") == property)?.Attribute("Value")?.Value;

    private static XElement Named(XElement scope, string name) =>
        scope.Descendants().Single(e => (string?)e.Attribute(X + "Name") == name);

    private static double[] Thickness(string? value)
    {
        var parts = value!.Split(',').Select(p => double.Parse(p, CultureInfo.InvariantCulture)).ToArray();
        return parts.Length switch
        {
            1 => new[] { parts[0], parts[0], parts[0], parts[0] },
            2 => new[] { parts[0], parts[1], parts[0], parts[1] },
            _ => parts,
        };
    }

    /// <summary>The element a card shows as its control, for every settings card in the app.</summary>
    private static List<(string file, XElement content)> CardContents() => Pages()
        .SelectMany(p => p.doc.Descendants(Ui + "CardControl")
            .SelectMany(card => card.Elements().Where(e => !e.Name.LocalName.Contains('.')))
            .Select(content => (p.file, content)))
        .ToList();

    private static string Describe(string file, XElement e) =>
        $"{file}: {e.Name.LocalName} {(string?)e.Attribute(X + "Name") ?? (string?)e.Attribute("IsChecked") ?? ""}".TrimEnd();

    [Fact]
    public void Fr17_a_toggle_switch_sits_against_the_card_right_edge()
    {
        var toggle = TypedStyle("ui:ToggleSwitch");
        Assert.Equal("Right", Setter(toggle, "HorizontalAlignment"));
        Assert.Equal("Center", Setter(toggle, "VerticalAlignment"));

        // A row of controls in one card is right-aligned too, so a switch at its end reaches the
        // edge whether or not the controls before it are shown.
        var row = KeyedStyle("CardControls");
        Assert.Equal("StackPanel", (string?)row.Attribute("TargetType"));
        Assert.Equal("Horizontal", Setter(row, "Orientation"));
        Assert.Equal("Right", Setter(row, "HorizontalAlignment"));
    }

    [Fact]
    public void Fr17_every_card_toggle_switch_uses_the_shared_placement()
    {
        var contents = CardContents();
        var toggles = contents.SelectMany(c => c.content.DescendantsAndSelf(Ui + "ToggleSwitch").Select(t => (c.file, c.content, t))).ToList();
        // The switches the screenshots showed out of line.
        Assert.Contains(toggles, t => t.file == "DisplayPage.xaml" && (string?)t.t.Attribute(X + "Name") == "TouchGesturesSwitch");
        Assert.Contains(toggles, t => t.file == "UpdatesPage.xaml" && (string?)t.t.Attribute(X + "Name") == "ImageSwitch");
        Assert.Contains(toggles, t => t.file == "AdvancedPage.xaml" && (string?)t.t.Attribute(X + "Name") == "ClipboardSwitch");
        Assert.Contains(toggles, t => t.file == "NetworkPage.xaml" && (string?)t.t.Attribute(X + "Name") == "AdbAutoConnectSwitch");

        var offenders = new List<string>();
        foreach (var (file, content, toggle) in toggles)
        {
            foreach (var own in new[] { "Style", "HorizontalAlignment", "Margin", "Width", "MinWidth" })
            {
                if (toggle.Attribute(own) is not null)
                {
                    offenders.Add($"{Describe(file, toggle)} sets its own {own}");
                }
            }
            // Every panel between the card and the switch is a CardControls row.
            for (var parent = toggle.Parent; parent is not null && toggle != content && parent != content.Parent; parent = parent.Parent)
            {
                if ((string?)parent.Attribute("Style") != CardControls)
                {
                    offenders.Add($"{Describe(file, toggle)} sits in a {parent.Name.LocalName} that is not a CardControls row");
                }
            }
        }
        Assert.True(offenders.Count == 0,
            "these switches do not use the shared placement, so they stop short of the card's right edge: "
            + string.Join("; ", offenders));
    }

    [Fact]
    public void Fr17_every_row_of_card_controls_sits_against_the_card_right_edge()
    {
        var offenders = CardContents()
            .Where(c => c.content.Name == Wpf + "StackPanel" && (string?)c.content.Attribute("Orientation") == "Horizontal"
                || c.content.Name == Wpf + "StackPanel" && c.content.Attribute("Style") is not null
                    && (string?)c.content.Attribute("Style") != CardControls)
            .Select(c => Describe(c.file, c.content))
            .Concat(CardContents()
                .Where(c => (string?)c.content.Attribute("Style") == CardControls)
                .Where(c => c.content.Attribute("HorizontalAlignment") is not null || c.content.Attribute("Orientation") is not null)
                .Select(c => Describe(c.file, c.content) + " overrides the row's placement"))
            .ToList();
        Assert.True(offenders.Count == 0,
            "these rows in a card's control box do not use the CardControls style: " + string.Join("; ", offenders));
    }

    [Fact]
    public void Fr17_a_notice_bar_lines_up_with_the_cards_icon_and_title_columns()
    {
        var card = TypedStyle("ui:CardControl");
        var bar = TypedStyle("ui:InfoBar");

        // The card: border, padding, then a 20 px icon with its margins, then the title.
        var cardBorder = Thickness(Setter(card, "BorderThickness"))[0];
        var cardPadding = Thickness(Setter(card, "Padding"))[0];
        var cardIcon = Named(card, "ControlIcon");
        var cardIconMargin = Thickness((string?)cardIcon.Attribute("Margin"));
        var cardIconWidth = double.Parse(((string?)cardIcon.Attribute("FontSize"))!, CultureInfo.InvariantCulture);

        // The bar: the same, with its icon centred in a box as wide as the card's icon.
        Assert.Equal("{TemplateBinding Padding}", (string?)Named(bar, "ContentBorder").Attribute("Padding"));
        Assert.Equal("{TemplateBinding BorderThickness}", (string?)Named(bar, "ContentBorder").Attribute("BorderThickness"));
        var barBorder = Thickness(Setter(bar, "BorderThickness"))[0];
        var barPadding = Thickness(Setter(bar, "Padding"));
        var iconBox = Named(bar, "IconBox");
        var iconBoxMargin = Thickness((string?)iconBox.Attribute("Margin"));
        var iconBoxWidth = double.Parse(((string?)iconBox.Attribute("Width"))!, CultureInfo.InvariantCulture);
        Assert.Equal("0", (string?)iconBox.Attribute("Grid.Column"));
        Assert.Equal("Center", (string?)Named(bar, "SymbolIcon").Attribute("HorizontalAlignment"));

        var cardIconLeft = cardBorder + cardPadding + cardIconMargin[0];
        var cardTitleLeft = cardIconLeft + cardIconWidth + cardIconMargin[2];
        var barIconLeft = barBorder + barPadding[0] + iconBoxMargin[0];
        var barTitleLeft = barIconLeft + iconBoxWidth + iconBoxMargin[2];
        Assert.Equal(19, cardIconLeft);
        Assert.Equal(59, cardTitleLeft);
        Assert.Equal(cardIconLeft, barIconLeft);
        Assert.Equal(cardIconWidth, iconBoxWidth);
        Assert.Equal(cardTitleLeft, barTitleLeft);
        // The right side matches the card's too, so a message wraps where a card's text does.
        Assert.Equal(cardPadding, barPadding[2]);

        // No bar on a page moves its own content.
        var offenders = Pages()
            .SelectMany(p => p.doc.Descendants(Ui + "InfoBar")
                .Where(b => b.Attribute("Padding") is not null || b.Attribute("Style") is not null
                    || b.Attribute("Template") is not null || b.Attribute("BorderThickness") is not null)
                .Select(b => $"{p.file}: {(string?)b.Attribute("Title")}"))
            .ToList();
        Assert.True(offenders.Count == 0, "these notice bars set their own padding, border or style: " + string.Join("; ", offenders));
    }
}

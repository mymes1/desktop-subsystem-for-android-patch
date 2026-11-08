using Xunit;
using System.Xml.Linq;

namespace Dsa.Settings.Core.Tests;

/// <summary>A plain ItemsControl puts each row of a page's list behind a generic data item, and
/// UI Automation's control view showed nothing inside it: the Widgets page read as its headings
/// alone, with every widget drawn on the screen and none reaching a screen reader or the
/// installer test. A page's lists are RowLists, whose automation peer exposes the rows' own
/// controls.</summary>
public class RowListTests
{
    private static readonly XNamespace Wpf = "http://schemas.microsoft.com/winfx/2006/xaml/presentation";

    private const string Local = "clr-namespace:Dsa.Settings";

    private static string SettingsApp()
    {
        // The tests run from dsa/target/settings/..., outside the source tree, so each folder
        // above is tried both as the settings folder and as the repository's dsa folder.
        string[] relative = { "DsaSettings", Path.Combine("host", "platform", "windows", "settings", "DsaSettings") };
        for (var dir = new DirectoryInfo(AppContext.BaseDirectory); dir is not null; dir = dir.Parent)
        {
            var found = relative.Select(r => Path.Combine(dir.FullName, r))
                .FirstOrDefault(p => File.Exists(Path.Combine(p, "App.xaml")));
            if (found is not null)
            {
                return found;
            }
        }
        throw new DirectoryNotFoundException("no settings app folder above " + AppContext.BaseDirectory);
    }

    private static XDocument Page(string name) =>
        XDocument.Load(Path.Combine(SettingsApp(), "Pages", name + ".xaml"));

    private static IEnumerable<string> PageNames() =>
        Directory.EnumerateFiles(Path.Combine(SettingsApp(), "Pages"), "*.xaml")
            .Select(file => Path.GetFileNameWithoutExtension(file));

    [Fact]
    public void Fr12_the_widgets_page_lists_placed_and_offered_widgets_where_ui_automation_reads_them()
    {
        var page = Page("WidgetsPage");
        var lists = page.Descendants(XName.Get("RowList", Local))
            .Select(e => (string?)e.Attribute("ItemsSource"))
            .ToList();
        Assert.Contains("{Binding Placed, Mode=OneWay}", lists);
        Assert.Contains("{Binding Offers, Mode=OneWay}", lists);
        Assert.Empty(page.Descendants(Wpf + "ItemsControl"));
    }

    [Fact]
    public void Fr17_no_settings_page_lists_rows_in_a_plain_items_control()
    {
        Assert.NotEmpty(PageNames());
        var plain = PageNames()
            .Where(name => Page(name).Descendants(Wpf + "ItemsControl").Any())
            .ToList();
        Assert.True(plain.Count == 0,
            "these pages list rows in a plain ItemsControl, whose rows UI Automation cannot read; use l:RowList: "
            + string.Join(", ", plain));
    }

    [Fact]
    public void Fr17_a_row_list_exposes_its_rows_controls_rather_than_data_items()
    {
        var source = File.ReadAllText(Path.Combine(SettingsApp(), "RowList.cs"));
        Assert.Contains("class RowList : ItemsControl", source);
        Assert.Contains("OnCreateAutomationPeer() => new RowListAutomationPeer(this)", source);
        // The element peer walks the visual tree; an ItemsControlAutomationPeer would put a data
        // item in front of every row again.
        Assert.Contains("class RowListAutomationPeer : FrameworkElementAutomationPeer", source);
        Assert.DoesNotContain("ItemsControlAutomationPeer", source);
    }
}

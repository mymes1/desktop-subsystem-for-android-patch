using System.Xml.Linq;
using Xunit;

namespace Dsa.Settings.Core.Tests;

/// <summary>The APK Installer's permissions and details were plain ItemsControls, which put each
/// row behind a generic data item, so UI Automation's control view showed none of the rows'
/// text: a screen reader and the installer test read "Capabilities (40):" with no permission
/// under it. The window's lists are the settings app's RowLists, whose automation peer exposes
/// the rows' own text.</summary>
public class ApkInstallerRowListTests
{
    private static readonly XNamespace Wpf = "http://schemas.microsoft.com/winfx/2006/xaml/presentation";
    private static readonly XNamespace X = "http://schemas.microsoft.com/winfx/2006/xaml";

    private static readonly XName RowList = XName.Get("RowList", "clr-namespace:Dsa.Settings");

    private static string InstallerApp()
    {
        // The tests run from dsa/target/settings/..., outside the source tree, so each folder
        // above is tried both as the settings folder and as the repository's dsa folder.
        string[] relative = { "DsaApkInstaller", Path.Combine("host", "platform", "windows", "settings", "DsaApkInstaller") };
        for (var dir = new DirectoryInfo(AppContext.BaseDirectory); dir is not null; dir = dir.Parent)
        {
            var found = relative.Select(r => Path.Combine(dir.FullName, r))
                .FirstOrDefault(p => File.Exists(Path.Combine(p, "InstallerWindow.xaml")));
            if (found is not null)
            {
                return found;
            }
        }
        throw new DirectoryNotFoundException("no APK Installer folder above " + AppContext.BaseDirectory);
    }

    private static IEnumerable<string> XamlFiles() =>
        Directory.EnumerateFiles(InstallerApp(), "*.xaml", SearchOption.AllDirectories);

    [Fact]
    public void Fr11_the_installer_lists_permissions_and_details_where_ui_automation_reads_them()
    {
        var window = XDocument.Load(Path.Combine(InstallerApp(), "InstallerWindow.xaml"));
        var lists = window.Descendants(RowList).Select(e => (string?)e.Attribute(X + "Name")).ToList();
        Assert.Contains("PermissionPreview", lists);
        Assert.Contains("PermissionList", lists);
        Assert.Contains("FactList", lists);
        // The installer test finds the lists by these ids.
        Assert.All(window.Descendants(RowList), e =>
            Assert.Equal((string?)e.Attribute(X + "Name"), (string?)e.Attribute("AutomationProperties.AutomationId")));
    }

    [Fact]
    public void Fr17_no_installer_window_lists_rows_in_a_plain_items_control()
    {
        Assert.NotEmpty(XamlFiles());
        var plain = XamlFiles()
            .Where(file => XDocument.Load(file).Descendants(Wpf + "ItemsControl").Any())
            .Select(Path.GetFileName)
            .ToList();
        Assert.True(plain.Count == 0,
            "these files list rows in a plain ItemsControl, whose rows UI Automation cannot read; use rows:RowList: "
            + string.Join(", ", plain));
    }

    [Fact]
    public void Fr17_the_installer_builds_the_settings_apps_row_list()
    {
        // One source for both apps, so the installer's lists cannot drift back to a peer that
        // puts data items in front of the rows while the settings app's stay readable.
        var project = XDocument.Load(Path.Combine(InstallerApp(), "DsaApkInstaller.csproj"));
        Assert.Contains(project.Descendants("Compile"),
            e => (string?)e.Attribute("Include") == @"..\DsaSettings\RowList.cs");
        Assert.False(File.Exists(Path.Combine(InstallerApp(), "RowList.cs")),
            "the APK Installer keeps its own RowList.cs; build the settings app's instead");
    }
}

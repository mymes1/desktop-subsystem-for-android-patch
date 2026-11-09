using System.Text.RegularExpressions;
using Xunit;

namespace Dsa.Settings.Core.Tests;

/// <summary>Every notice bar in the settings app and the APK Installer has a title. A bar with
/// only a message starts its text where the other bars' titles start, so its lines do not line
/// up with the bars around it: the Display page's "the render GPU applies when the window
/// manager starts again" bar looked out of place beside the titled ones.</summary>
public class InfoBarTitleTests
{
    private static string SettingsRoot()
    {
        string[] relative = { "DsaSettings", Path.Combine("host", "platform", "windows", "settings", "DsaSettings") };
        for (var dir = new DirectoryInfo(AppContext.BaseDirectory); dir is not null; dir = dir.Parent)
        {
            var found = relative.Select(r => Path.Combine(dir.FullName, r))
                .FirstOrDefault(d => File.Exists(Path.Combine(d, "DsaSettings.csproj")));
            if (found is not null)
            {
                return Path.GetDirectoryName(found)!;
            }
        }
        Assert.Fail("the settings app's source was not found above " + AppContext.BaseDirectory);
        return "";
    }

    [Fact]
    public void Fr17_every_notice_bar_has_a_title()
    {
        var root = SettingsRoot();
        var bar = new Regex(@"<ui:InfoBar\b[^>]*>", RegexOptions.Singleline);
        var untitled = new[] { "DsaSettings", "DsaApkInstaller" }
            .SelectMany(app => Directory.GetFiles(Path.Combine(root, app), "*.xaml", SearchOption.AllDirectories))
            .Where(file => !file.Contains(Path.DirectorySeparatorChar + "obj" + Path.DirectorySeparatorChar)
                && !file.Contains(Path.DirectorySeparatorChar + "bin" + Path.DirectorySeparatorChar))
            .SelectMany(file => bar.Matches(File.ReadAllText(file))
                .Where(m => !Regex.IsMatch(m.Value, @"\bTitle\s*="))
                .Select(m => Path.GetFileName(file) + ": " + Regex.Replace(m.Value, @"\s+", " ")))
            .ToList();
        Assert.True(untitled.Count == 0, "these notice bars have no title: " + string.Join(" | ", untitled));
    }
}

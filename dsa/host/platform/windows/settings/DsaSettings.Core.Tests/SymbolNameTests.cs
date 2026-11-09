using System.Reflection;
using System.Reflection.Metadata;
using System.Reflection.PortableExecutable;
using System.Text.RegularExpressions;
using Xunit;

namespace Dsa.Settings.Core.Tests;

/// <summary>Every icon the settings app and the APK Installer name in XAML or code exists in
/// WPF UI 3.0.5. XAML turns the name into the library's symbol when the element is made, so a
/// name the library lacks fails the page that holds it the moment it opens: the Advanced page
/// with "Store24" threw before it showed.</summary>
public class SymbolNameTests
{
    private static HashSet<string> LibrarySymbols()
    {
        var path = typeof(SymbolNameTests).Assembly.GetCustomAttributes<AssemblyMetadataAttribute>()
            .Single(a => a.Key == "WpfUiAssembly").Value!;
        Assert.True(File.Exists(path), "WPF UI 3.0.5 was not found at " + path);
        using var stream = File.OpenRead(path);
        using var pe = new PEReader(stream);
        var metadata = pe.GetMetadataReader();
        var symbols = metadata.TypeDefinitions.Select(metadata.GetTypeDefinition)
            .Single(t => metadata.GetString(t.Name) == "SymbolRegular"
                && metadata.GetString(t.Namespace) == "Wpf.Ui.Controls");
        return symbols.GetFields().Select(metadata.GetFieldDefinition)
            .Where(f => (f.Attributes & FieldAttributes.Static) != 0)
            .Select(f => metadata.GetString(f.Name))
            .ToHashSet();
    }

    /// <summary>Each SymbolRegular name with its code point, read from the enum's constants.</summary>
    private static Dictionary<string, int> LibraryCodePoints()
    {
        var path = typeof(SymbolNameTests).Assembly.GetCustomAttributes<AssemblyMetadataAttribute>()
            .Single(a => a.Key == "WpfUiAssembly").Value!;
        using var stream = File.OpenRead(path);
        using var pe = new PEReader(stream);
        var metadata = pe.GetMetadataReader();
        var symbols = metadata.TypeDefinitions.Select(metadata.GetTypeDefinition)
            .Single(t => metadata.GetString(t.Name) == "SymbolRegular"
                && metadata.GetString(t.Namespace) == "Wpf.Ui.Controls");
        var points = new Dictionary<string, int>();
        foreach (var field in symbols.GetFields().Select(metadata.GetFieldDefinition))
        {
            var constant = field.GetDefaultValue();
            if ((field.Attributes & FieldAttributes.Static) == 0 || constant.IsNil)
            {
                continue;
            }
            var value = metadata.GetBlobReader(metadata.GetConstant(constant).Value);
            points[metadata.GetString(field.Name)] = value.Length >= 4 ? value.ReadInt32() : value.ReadUInt16();
        }
        return points;
    }

    private static string SettingsRoot()
    {
        // The tests run from dsa/target/settings/..., outside the source tree, so each folder
        // above is tried both as the settings folder and as the repository's dsa folder.
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
    public void Fr17_every_icon_the_apps_name_is_a_wpf_ui_symbol()
    {
        var symbols = LibrarySymbols();
        Assert.Contains("Delete24", symbols);
        Assert.DoesNotContain("Store24", symbols);

        var root = SettingsRoot();
        var name = new Regex(@"\{ui:SymbolIcon\s+(?:Symbol=)?(\w+)|\bSymbol=""(\w+)""|SymbolRegular\.(\w+)");
        var missing = new[] { "DsaSettings", "DsaApkInstaller" }
            .SelectMany(app => Directory.GetFiles(Path.Combine(root, app), "*.*", SearchOption.AllDirectories))
            .Where(file => file.EndsWith(".xaml", StringComparison.Ordinal) || file.EndsWith(".cs", StringComparison.Ordinal))
            .Where(file => !file.Contains(Path.DirectorySeparatorChar + "obj" + Path.DirectorySeparatorChar)
                && !file.Contains(Path.DirectorySeparatorChar + "bin" + Path.DirectorySeparatorChar))
            .SelectMany(file => name.Matches(File.ReadAllText(file))
                .Select(m => (file: Path.GetFileName(file),
                    symbol: m.Groups.Values.Skip(1).First(g => g.Success).Value)))
            .Where(use => !symbols.Contains(use.symbol))
            .Select(use => $"{use.file}: {use.symbol}")
            .Distinct()
            .ToList();
        Assert.True(missing.Count == 0,
            "these icons are not in WPF UI 3.0.5, so the element that names them fails when it is made: "
            + string.Join(", ", missing));
    }

    [Fact]
    public void Fr17_every_icon_the_apps_name_fits_in_one_utf16_unit()
    {
        // WPF UI 3.0.5 turns a symbol into one char, so a code point above U+FFFF loses its
        // top bits and draws another character: ArrowRouting24 (U+F01E7) drew U+01E7, a "ǧ",
        // on the Updates page.
        var points = LibraryCodePoints();
        Assert.True(points["ArrowRouting24"] > 0xFFFF);
        Assert.True(points["Delete24"] <= 0xFFFF);

        var root = SettingsRoot();
        var name = new Regex(@"\{ui:SymbolIcon\s+(?:Symbol=)?(\w+)|\bSymbol=""(\w+)""|SymbolRegular\.(\w+)");
        var wide = new[] { "DsaSettings", "DsaApkInstaller" }
            .SelectMany(app => Directory.GetFiles(Path.Combine(root, app), "*.*", SearchOption.AllDirectories))
            .Where(file => file.EndsWith(".xaml", StringComparison.Ordinal) || file.EndsWith(".cs", StringComparison.Ordinal))
            .Where(file => !file.Contains(Path.DirectorySeparatorChar + "obj" + Path.DirectorySeparatorChar)
                && !file.Contains(Path.DirectorySeparatorChar + "bin" + Path.DirectorySeparatorChar))
            .SelectMany(file => name.Matches(File.ReadAllText(file))
                .Select(m => (file: Path.GetFileName(file),
                    symbol: m.Groups.Values.Skip(1).First(g => g.Success).Value)))
            .Where(use => points.TryGetValue(use.symbol, out var point) && point > 0xFFFF)
            .Select(use => $"{use.file}: {use.symbol} (U+{points[use.symbol]:X})")
            .Distinct()
            .ToList();
        Assert.True(wide.Count == 0,
            "these icons' code points do not fit in one UTF-16 unit, so WPF UI draws another character: "
            + string.Join(", ", wide));
    }
}

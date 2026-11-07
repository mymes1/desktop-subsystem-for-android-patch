using System.Text.RegularExpressions;

namespace Dsa.Settings.Core;

/// <summary>What the settings app is asked to open with. A task window's App info starts it as
/// <c>DsaSettings.exe --page apps --app com.example.notes</c>, which opens the Apps page with
/// that app's row in view.</summary>
public static class StartArguments
{
    /// <summary>The argument that names the app whose row the Apps page brings into view.</summary>
    public const string AppArgument = "--app";

    /// <summary>An Android package name: two or more dot-separated parts, each a letter followed
    /// by letters, digits and underscores.</summary>
    private static readonly Regex PackageName = new(@"^[A-Za-z][A-Za-z0-9_]*(\.[A-Za-z][A-Za-z0-9_]*)+$");

    /// <summary>The app <c>--app</c> names in <paramref name="args"/>, or null when there is
    /// none or it names no package.</summary>
    public static string? AppOf(IReadOnlyList<string> args)
    {
        for (var i = 0; i + 1 < args.Count; i++)
        {
            if (args[i] == AppArgument)
            {
                var package = args[i + 1];
                return package.Length <= 255 && PackageName.IsMatch(package) ? package : null;
            }
        }
        return null;
    }
}

using System.Globalization;
using System.Resources;
using Dsa.Settings.Core;

namespace Dsa.ApkInstaller;

/// <summary>The texts in the user's language, from the settings app's string tables, which
/// this app embeds as its own (Strings.resx in English, Strings.&lt;language&gt;.resx).</summary>
internal sealed class ResourceText : ITextSource
{
    private static readonly ResourceManager Manager =
        new("Dsa.ApkInstaller.Strings", typeof(ResourceText).Assembly);

    public string Get(string key)
    {
        try
        {
            return Manager.GetString(key, CultureInfo.CurrentUICulture) ?? key;
        }
        catch (MissingManifestResourceException)
        {
            return key;
        }
    }
}

using System.IO.Compression;

namespace Dsa.Settings.Core;

/// <summary>What a file the APK Installer cannot install is.</summary>
public enum UnsupportedKind
{
    /// <summary>An app split into several APKs and packed together: <c>.xapk</c>, <c>.apks</c>
    /// or <c>.apkm</c>.</summary>
    SplitBundle,
    /// <summary>An Android App Bundle (<c>.aab</c>), the format apps are uploaded to stores in.</summary>
    AppBundle,
    /// <summary>Anything else that is not an APK.</summary>
    NotApk,
}

/// <summary>
/// Says in plain words why a file cannot be installed. What the file is comes from what is
/// in it where it can be read, so a renamed bundle is still named for what it is, and from its
/// extension otherwise. The window is English in every language, so these are not in the
/// string tables.
/// </summary>
public static class UnsupportedFile
{
    public static UnsupportedKind Classify(string path)
    {
        if (FromContent(path) is { } kind)
        {
            return kind;
        }
        return System.IO.Path.GetExtension(path).ToLowerInvariant() switch
        {
            ".xapk" or ".apks" or ".apkm" => UnsupportedKind.SplitBundle,
            ".aab" => UnsupportedKind.AppBundle,
            _ => UnsupportedKind.NotApk,
        };
    }

    /// <summary>What the archive holds says what it is: an App Bundle has its BundleConfig.pb
    /// and its base module; a split bundle holds APKs, with toc.pb (.apks), info.json (.apkm)
    /// or manifest.json (.xapk) beside them. Null when the file is not an archive that says.</summary>
    private static UnsupportedKind? FromContent(string path)
    {
        try
        {
            using var zip = ZipFile.OpenRead(path);
            var names = zip.Entries.Select(e => e.FullName.Replace('\\', '/')).ToList();
            if (names.Any(n => n == "BundleConfig.pb" || n == "base/manifest/AndroidManifest.xml"))
            {
                return UnsupportedKind.AppBundle;
            }
            var bundled = names.Any(n => n.EndsWith(".apk", StringComparison.OrdinalIgnoreCase));
            return bundled || names.Contains("toc.pb") ? UnsupportedKind.SplitBundle : null;
        }
        catch (Exception e) when (e is InvalidDataException or IOException or UnauthorizedAccessException
            or NotSupportedException or ArgumentException)
        {
            return null;
        }
    }

    /// <summary>The sentence the window shows for <paramref name="fileName"/>.</summary>
    public static string Message(UnsupportedKind kind, string fileName) => kind switch
    {
        UnsupportedKind.SplitBundle =>
            $"{fileName} is a split APK bundle, an app packed as several APKs. Bundles like this are not supported yet, so only single APK files can be installed.",
        UnsupportedKind.AppBundle =>
            $"{fileName} is an Android App Bundle. App Bundles are a publishing format for app stores and cannot be installed directly. Get the APK from the app's publisher.",
        _ => $"{fileName} is not a valid APK file.",
    };
}

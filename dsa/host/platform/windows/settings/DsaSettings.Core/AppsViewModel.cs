using System.Globalization;
using System.Text.Json;
using System.Text.RegularExpressions;

namespace Dsa.Settings.Core;

/// <summary>One app installed in Android, as dsa apps list reports it.</summary>
public sealed record InstalledAppInfo(
    string Package,
    string Label,
    string VersionName,
    long VersionCode,
    ulong FirstInstallMs,
    ulong LastUpdateMs,
    bool SystemApp,
    ulong AppBytes,
    ulong DataBytes,
    ulong CacheBytes,
    byte[] IconPng)
{
    public ulong TotalBytes => AppBytes + DataBytes + CacheBytes;
}

/// <summary>How an uninstall went: <paramref name="Status"/> is dsa's stable reason
/// (<c>uninstalled</c>, <c>not-installed</c>, <c>system-app</c>, <c>blocked</c> or <c>other</c>),
/// <paramref name="Message"/> Android's own words.</summary>
public sealed record UninstallReport(bool Uninstalled, string Status, string Message);

/// <summary>Asks dsa about the apps installed in Android, and acts on them: opening one,
/// opening Android's settings of one or its list of apps, and uninstalling one.</summary>
public sealed class AppsService
{
    private readonly IDsaCli _cli;

    public AppsService(IDsaCli cli)
    {
        _cli = cli;
    }

    /// <summary>The start of dsa's line for an app Android did not uninstall: the reason's
    /// code, then the detail.</summary>
    private static readonly Regex Refusal = new(@"^not uninstalled \(([a-z-]+)\): (.*)$", RegexOptions.Singleline);

    private static string Detail(CliResult result) =>
        result.Error.StartsWith("dsa: ", StringComparison.Ordinal) ? result.Error["dsa: ".Length..] : result.Error;

    private static DsaException Failure(CliResult result)
    {
        var detail = Detail(result);
        var problem = detail.Contains("cannot reach the DSA service", StringComparison.Ordinal)
            ? Problem.ServiceUnreachable
            : Problem.Refused;
        return new DsaException(problem, detail.Length > 0 ? detail : $"dsa ended with code {result.ExitCode}");
    }

    private async Task<JsonElement> RunJsonAsync(params string[] args)
    {
        var all = new List<string>(args) { "--json" };
        var result = await _cli.RunAsync(all).ConfigureAwait(true);
        if (result.ExitCode != 0)
        {
            throw Failure(result);
        }
        return Parse(result.Output);
    }

    private static JsonElement Parse(string output)
    {
        try
        {
            using var document = JsonDocument.Parse(output);
            return document.RootElement.Clone();
        }
        catch (JsonException e)
        {
            throw new DsaException(Problem.Unreadable, $"dsa printed something other than JSON: {e.Message}");
        }
    }

    private static string String(JsonElement json, string name) =>
        json.TryGetProperty(name, out var value) && value.ValueKind == JsonValueKind.String ? value.GetString() ?? "" : "";

    private static ulong Number(JsonElement json, string name) =>
        json.TryGetProperty(name, out var value) && value.ValueKind == JsonValueKind.Number && value.TryGetUInt64(out var n) ? n : 0;

    private static long Signed(JsonElement json, string name) =>
        json.TryGetProperty(name, out var value) && value.ValueKind == JsonValueKind.Number && value.TryGetInt64(out var n) ? n : 0;

    private static bool Bool(JsonElement json, string name) =>
        json.TryGetProperty(name, out var value) && value.ValueKind == JsonValueKind.True;

    private static byte[] Png(JsonElement json, string name)
    {
        try
        {
            return Convert.FromBase64String(String(json, name));
        }
        catch (FormatException)
        {
            return Array.Empty<byte>();
        }
    }

    public static InstalledAppInfo ReadApp(JsonElement app) => new(
        String(app, "package"),
        String(app, "label"),
        String(app, "version_name"),
        Signed(app, "version_code"),
        Number(app, "first_install_time_ms"),
        Number(app, "last_update_time_ms"),
        Bool(app, "system_app"),
        Number(app, "app_bytes"),
        Number(app, "data_bytes"),
        Number(app, "cache_bytes"),
        Png(app, "icon_png"));

    /// <summary>Every app installed in Android, with its size read again; Android is started
    /// first when it is not running.</summary>
    public async Task<IReadOnlyList<InstalledAppInfo>> ListAsync()
    {
        var json = await RunJsonAsync("apps", "list").ConfigureAwait(true);
        var apps = new List<InstalledAppInfo>();
        if (json.TryGetProperty("apps", out var list) && list.ValueKind == JsonValueKind.Array)
        {
            foreach (var app in list.EnumerateArray())
            {
                var read = ReadApp(app);
                if (read.Package.Length > 0)
                {
                    apps.Add(read);
                }
            }
        }
        return apps;
    }

    /// <summary>Opens the app in a window of its own.</summary>
    public async Task OpenAsync(string package)
    {
        var result = await _cli.RunAsync(new[] { "launch", package }).ConfigureAwait(true);
        if (result.ExitCode != 0)
        {
            throw Failure(result);
        }
    }

    /// <summary>Opens Android's own settings of <paramref name="package"/> (its permissions,
    /// storage and notifications), or Android's list of every app when it is null.</summary>
    public async Task OpenSettingsAsync(string? package)
    {
        if (package is null)
        {
            await RunJsonAsync("apps", "settings").ConfigureAwait(true);
        }
        else
        {
            await RunJsonAsync("apps", "settings", package).ConfigureAwait(true);
        }
    }

    /// <summary>Uninstalls <paramref name="package"/>. A refusal by Android is a report with its
    /// reason; anything else that kept the request from Android is a
    /// <see cref="DsaException"/>.</summary>
    public async Task<UninstallReport> UninstallAsync(string package)
    {
        var result = await _cli.RunAsync(new[] { "apps", "uninstall", package, "--json" }).ConfigureAwait(true);
        if (result.ExitCode != 0)
        {
            var refused = Refusal.Match(Detail(result));
            if (refused.Success)
            {
                return new UninstallReport(false, refused.Groups[1].Value, refused.Groups[2].Value);
            }
            throw Failure(result);
        }
        var json = Parse(result.Output);
        return new UninstallReport(Bool(json, "uninstalled"), String(json, "status"), String(json, "message"));
    }
}

/// <summary>One app on the Apps page.</summary>
public sealed record AppRow(
    string Package,
    string Label,
    string Detail,
    string Size,
    byte[]? Icon,
    bool CanUninstall,
    bool IsSystem,
    string? UninstallTip)
{
    public bool NoIcon => Icon is null;
}

/// <summary>The Apps page: every app installed in Android, searchable, each to open, to see
/// in Android's own settings, or to uninstall; and Android's own list of apps.</summary>
public sealed class AppsViewModel : ObservableObject
{
    private readonly AppsService _service;
    private readonly ITextSource _text;
    private IReadOnlyList<InstalledAppInfo>? _apps;
    private string _search = "";
    private string? _error;
    private string? _message;
    private bool _busy;

    public AppsViewModel(AppsService service, ITextSource text)
    {
        _service = service;
        _text = text;
    }

    public bool IsBusy => _busy;
    public bool IsIdle => !_busy;
    public string Error => _error ?? "";
    public bool HasError => _error is not null;
    public string Message => _message ?? "";
    public bool HasMessage => _message is not null;
    public bool HasNoApps => _apps is { Count: 0 };
    public bool HasNoMatches => _apps is { Count: > 0 } && Rows.Count == 0;

    /// <summary>How many apps are installed, and how many the search shows.</summary>
    public string Count => _apps is null
        ? ""
        : _search.Trim().Length == 0
            ? string.Format(CultureInfo.CurrentCulture, _text.Get("Apps_Count"), _apps.Count)
            : string.Format(CultureInfo.CurrentCulture, _text.Get("Apps_CountMatching"), Rows.Count, _apps.Count);

    /// <summary>What the user typed to find an app: matched against its name and package.</summary>
    public string Search
    {
        get => _search;
        set
        {
            if (_search == value)
            {
                return;
            }
            _search = value ?? "";
            Changed();
            Changed(nameof(Rows));
            Changed(nameof(HasNoMatches));
            Changed(nameof(Count));
        }
    }

    /// <summary>A date in milliseconds since the Unix epoch, in the user's short date format.</summary>
    public static string Date(ulong ms, CultureInfo? culture = null) =>
        DateTimeOffset.FromUnixTimeMilliseconds((long)Math.Min(ms, (ulong)long.MaxValue))
            .ToLocalTime()
            .ToString("d", culture ?? CultureInfo.CurrentCulture);

    /// <summary>A size in bytes, as the Memory page shows sizes.</summary>
    public static string Size(ulong bytes, CultureInfo? culture = null) =>
        bytes < 1024 * 1024 && bytes > 0
            ? string.Format(culture ?? CultureInfo.CurrentCulture, "{0:0} KB", Math.Ceiling(bytes / 1024.0))
            : Format.Size(bytes / 1024, culture);

    /// <summary>The apps the search matches, by name.</summary>
    public IReadOnlyList<AppRow> Rows
    {
        get
        {
            var query = _search.Trim();
            return (_apps ?? Array.Empty<InstalledAppInfo>())
                .Where(a => query.Length == 0
                    || a.Label.Contains(query, StringComparison.CurrentCultureIgnoreCase)
                    || a.Package.Contains(query, StringComparison.OrdinalIgnoreCase))
                .OrderBy(a => a.Label.Length > 0 ? a.Label : a.Package, StringComparer.CurrentCultureIgnoreCase)
                .Select(Row)
                .ToList();
        }
    }

    private AppRow Row(InstalledAppInfo app)
    {
        var parts = new List<string> { app.Package };
        if (app.VersionName.Length > 0)
        {
            parts.Add(string.Format(CultureInfo.CurrentCulture, _text.Get("Apps_Version"), app.VersionName));
        }
        if (app.FirstInstallMs > 0)
        {
            parts.Add(string.Format(CultureInfo.CurrentCulture, _text.Get("Apps_Installed"), Date(app.FirstInstallMs)));
        }
        if (app.SystemApp)
        {
            parts.Add(_text.Get("Apps_System"));
        }
        return new AppRow(
            app.Package,
            app.Label.Length > 0 ? app.Label : app.Package,
            string.Join(" · ", parts),
            Size(app.TotalBytes),
            app.IconPng.Length > 0 ? app.IconPng : null,
            !app.SystemApp,
            app.SystemApp,
            app.SystemApp ? _text.Get("Apps_SystemTip") : null);
    }

    /// <summary>The name an app goes by on the page.</summary>
    public string LabelOf(string package) =>
        _apps?.FirstOrDefault(a => a.Package == package) is { Label.Length: > 0 } app ? app.Label : package;

    private async Task Run(Func<Task<string?>> work, bool reload)
    {
        if (_busy)
        {
            return;
        }
        _busy = true;
        _message = null;
        ChangedAll();
        try
        {
            _message = await work().ConfigureAwait(true);
            if (reload)
            {
                _apps = await _service.ListAsync().ConfigureAwait(true);
            }
            _error = null;
        }
        catch (RefusedByAndroid e)
        {
            _error = e.Message;
        }
        catch (Exception e)
        {
            _error = Format.Problem(_text, e);
        }
        finally
        {
            _busy = false;
        }
        ChangedAll();
    }

    /// <summary>Reads the installed apps.</summary>
    public Task RefreshAsync() => Run(() => Task.FromResult<string?>(null), reload: true);

    /// <summary>Opens the app in a window of its own.</summary>
    public Task OpenAsync(string package) => Run(async () =>
    {
        await _service.OpenAsync(package).ConfigureAwait(true);
        return null;
    }, reload: false);

    /// <summary>Opens Android's own page of the app: its permissions, storage and
    /// notifications.</summary>
    public Task InfoAsync(string package) => Run(async () =>
    {
        await _service.OpenSettingsAsync(package).ConfigureAwait(true);
        return null;
    }, reload: false);

    /// <summary>Opens Android's own list of every app.</summary>
    public Task ManageAsync() => Run(async () =>
    {
        await _service.OpenSettingsAsync(null).ConfigureAwait(true);
        return null;
    }, reload: false);

    /// <summary>Uninstalls the app; the page asks the user first. A refusal is the page's
    /// error, in the user's language.</summary>
    public Task UninstallAsync(string package) => Run(async () =>
    {
        var label = LabelOf(package);
        var report = await _service.UninstallAsync(package).ConfigureAwait(true);
        if (!report.Uninstalled)
        {
            var reason = _text.Get($"Apps_Refused_{report.Status}");
            throw new RefusedByAndroid(
                string.Format(CultureInfo.CurrentCulture, _text.Get("Apps_NotUninstalled"), label, reason, report.Message));
        }
        return string.Format(CultureInfo.CurrentCulture, _text.Get("Apps_Uninstalled"), label);
    }, reload: true);

    /// <summary>Android refused; the message says why in the user's language.</summary>
    private sealed class RefusedByAndroid : Exception
    {
        public RefusedByAndroid(string message) : base(message)
        {
        }
    }
}

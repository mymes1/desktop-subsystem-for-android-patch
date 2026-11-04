using System.Globalization;
using System.Text.Json;

namespace Dsa.Settings.Core;

/// <summary>Which screen the uninstall window shows.</summary>
public enum UninstallPage
{
    /// <summary>Android is uninstalling the app.</summary>
    Progress,
    Done,
    Failed,
}

/// <summary>The window Windows' list of installed apps opens to uninstall an Android app.
/// Windows has already asked the user, so it does not ask again: it shows the app and the
/// progress, uninstalls at once, then says how it ended. It runs dsa hidden, so no console
/// window shows.</summary>
public sealed class UninstallAppViewModel : ObservableObject
{
    private readonly IDsaCli _cli;
    private readonly AppsService _apps;
    private readonly ITextSource _text;
    private UninstallPage _page = UninstallPage.Progress;
    private InstalledAppInfo? _app;
    private string _resultText = "";
    private bool _started;

    public UninstallAppViewModel(IDsaCli cli, ITextSource text, string package)
    {
        _cli = cli;
        _apps = new AppsService(cli);
        _text = text;
        Package = package;
    }

    public string Package { get; }
    public UninstallPage Page => _page;
    public InstalledAppInfo? App => _app;
    public bool IsBusy => _page == UninstallPage.Progress;

    /// <summary>Whether the window closes by itself: after a successful uninstall.</summary>
    public bool ClosesItself => _page == UninstallPage.Done;

    /// <summary>The app's name, or its package until Android has said it.</summary>
    public string Title => _app is { Label.Length: > 0 } app ? app.Label : Package;

    public string VersionChip => _app is { VersionName.Length: > 0 } app
        ? string.Format(CultureInfo.CurrentCulture, _text.Get("UninstallWindow_Version"), app.VersionName)
        : "";

    public string SizeChip => _app is { TotalBytes: > 0 } app ? AppsViewModel.Size(app.TotalBytes) : "";

    public string ProgressTitle => string.Format(CultureInfo.CurrentCulture, _text.Get("UninstallWindow_Busy"), Title);

    public string ResultTitle => _text.Get(_page == UninstallPage.Done ? "UninstallWindow_DoneTitle" : "UninstallWindow_FailedTitle");

    public string ResultText => _resultText;

    private void Show(UninstallPage page)
    {
        _page = page;
        ChangedAll();
    }

    private string Refused(string status, string detail)
    {
        var reason = _text.Get($"Apps_Refused_{status}");
        return detail.Length == 0
            ? string.Format(CultureInfo.CurrentCulture, _text.Get("UninstallWindow_Refused"), Title, reason)
            : string.Format(CultureInfo.CurrentCulture, _text.Get("UninstallWindow_RefusedDetail"), Title, reason, detail);
    }

    /// <summary>Reads the app's name, icon and version to show, then uninstalls it in Android,
    /// starting Android first when it is not running. Runs once.</summary>
    public async Task RunAsync()
    {
        if (_started)
        {
            return;
        }
        _started = true;
        try
        {
            var result = await _cli.RunAsync(new[] { "apps", "info", Package, "--json" }).ConfigureAwait(true);
            if (result.ExitCode == 0)
            {
                using var document = JsonDocument.Parse(result.Output);
                var app = AppsService.ReadApp(document.RootElement);
                _app = app.Package.Length > 0 ? app : null;
            }
        }
        catch (Exception e) when (e is JsonException or DsaException)
        {
            // What the app is only helps to show it; the uninstall still goes ahead.
            _app = null;
        }
        ChangedAll();
        if (_app is { SystemApp: true })
        {
            _resultText = Refused("system-app", "");
            Show(UninstallPage.Failed);
            return;
        }
        try
        {
            var report = await _apps.UninstallAsync(Package).ConfigureAwait(true);
            if (report.Uninstalled)
            {
                _resultText = string.Format(CultureInfo.CurrentCulture, _text.Get("UninstallWindow_DoneText"), Title);
                Show(UninstallPage.Done);
            }
            else
            {
                _resultText = Refused(report.Status, report.Message);
                Show(UninstallPage.Failed);
            }
        }
        catch (Exception e)
        {
            _resultText = Format.Problem(_text, e);
            Show(UninstallPage.Failed);
        }
    }
}

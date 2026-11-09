using System.Globalization;

namespace Dsa.Settings.Core;

/// <summary>Root access: whether apps in Android may ask for root through an administrator's
/// approval. The computer's setting, so only an administrator changes it, and it applies from
/// the next start of Android. Each app's level is on the Apps page.</summary>
public sealed partial class SettingsViewModel
{
    private bool? _rootElevationAtStart;

    /// <summary>Whether apps in Android may ask for root. Off by default.</summary>
    public bool RootElevation => _settings?.RootElevation ?? false;

    /// <summary>The switch moves only for an administrator: it applies to every user of this
    /// PC.</summary>
    public bool CanChangeRootElevation => Loaded && !_busy && _administrator;

    /// <summary>Whether the switch can be changed only by running dsa as an administrator, so
    /// the page says why it does not move and offers Change as administrator.</summary>
    public bool RootElevationNeedsAdministrator => Loaded && !_busy && !_administrator;

    /// <summary>Whether the switch differs from what it was when the app first read it. The
    /// change applies from the next start of Android, which the page says.</summary>
    public bool RootElevationRestartNeeded =>
        Loaded && _rootElevationAtStart is { } before && before != RootElevation;

    /// <summary>Where each request and its result is recorded: the service log.</summary>
    public string RootElevationLog => HasLogDir
        ? string.Format(CultureInfo.CurrentCulture, _text.Get("RootElevation_LogAt"), LogDir)
        : _text.Get("RootElevation_LogUnknown");

    private void NoteRootElevation(SettingsInfo settings) =>
        _rootElevationAtStart ??= settings.RootElevation;

    public Task SetRootElevationAsync(bool on) =>
        on == RootElevation && Loaded ? Task.CompletedTask : ChangeAsync(() => _service.SetRootElevationAsync(on));
}

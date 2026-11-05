using System.Globalization;

namespace Dsa.Settings.Core;

/// <summary>One choice of how long Android waits, as needed, before it stops: what the list
/// shows and the wait in seconds.</summary>
public sealed record StopAfterChoice(string Label, int Secs);

/// <summary>The run mode, and resetting Android's data.</summary>
public sealed partial class SettingsViewModel
{
    private static readonly int[] OfferedStopAfterSecs = { 60, 300, 600, 900, 1800, 3600 };

    private bool _resetting;
    private bool _resetDone;

    /// <summary>Whether Android keeps running in Doze (continuous) instead of stopping once
    /// nothing uses it (as needed, the default).</summary>
    public bool Continuous => _settings?.RunMode == SettingsInfo.Continuous;

    /// <summary>How long nothing may use Android as needed before it stops, in seconds.</summary>
    public int StopAfterSecs => _settings?.StopAfterSecs ?? SettingsInfo.DefaultStopAfterSecs;

    /// <summary>Whether an app with a notification shown keeps Android running as needed.</summary>
    public bool KeepForNotifications => _settings?.KeepForNotifications ?? false;

    /// <summary>The wait and the notification switch only mean something as needed.</summary>
    public bool CanChangeStopAfter => CanChange && !Continuous;

    private string WaitText(int secs) => secs % 60 == 0
        ? string.Format(CultureInfo.CurrentCulture, _text.Get(secs == 60 ? "StopAfter_Minute" : "StopAfter_Minutes"), secs / 60)
        : string.Format(CultureInfo.CurrentCulture, _text.Get("StopAfter_Seconds"), secs);

    /// <summary>The waits on offer, and a wait set elsewhere (with <c>dsa settings set
    /// stop-after</c>) that is none of them, shortest first.</summary>
    public IReadOnlyList<StopAfterChoice> StopAfterChoices
    {
        get
        {
            var secs = new List<int>(OfferedStopAfterSecs);
            if (!secs.Contains(StopAfterSecs))
            {
                secs.Add(StopAfterSecs);
                secs.Sort();
            }
            return secs.Select(s => new StopAfterChoice(WaitText(s), s)).ToList();
        }
    }

    public int SelectedStopAfterIndex
    {
        get
        {
            var choices = StopAfterChoices;
            for (var i = 0; i < choices.Count; i++)
            {
                if (choices[i].Secs == StopAfterSecs)
                {
                    return i;
                }
            }
            return 0;
        }
    }

    public Task SetContinuousAsync(bool on) =>
        on == Continuous && Loaded
            ? Task.CompletedTask
            : ChangeAsync(() => _service.SetRunModeAsync(on ? SettingsInfo.Continuous : SettingsInfo.AsNeeded));

    /// <summary>Sets the wait, in seconds. A wait under 10 seconds or over a day changes
    /// nothing, as the service would not keep it.</summary>
    public Task SetStopAfterAsync(int secs)
    {
        if (secs is < 10 or > 86400)
        {
            ChangedAll();
            return Task.CompletedTask;
        }
        return secs == StopAfterSecs && Loaded ? Task.CompletedTask : ChangeAsync(() => _service.SetStopAfterAsync(secs));
    }

    public Task SetKeepForNotificationsAsync(bool on) =>
        on == KeepForNotifications && Loaded
            ? Task.CompletedTask
            : ChangeAsync(() => _service.SetKeepForNotificationsAsync(on));

    /// <summary>What the confirmation of a data reset says is lost, and what stays.</summary>
    public string ResetWarning => _text.Get("ResetData_Warning");

    /// <summary>Whether a reset is under way.</summary>
    public bool IsResetting => _resetting;

    /// <summary>Whether the last reset finished, so the page can say the data is gone.</summary>
    public bool ResetDone => _resetDone;

    public bool CanResetData => !_busy && !_resetting;

    /// <summary>Erases Android's data through the service. The page asks the user first, with
    /// <see cref="ResetWarning"/>; this does not ask again.</summary>
    public async Task ResetDataAsync()
    {
        _resetting = true;
        _resetDone = false;
        _error = null;
        ChangedAll();
        try
        {
            await _service.ResetDataAsync().ConfigureAwait(true);
            _resetDone = true;
        }
        catch (Exception e)
        {
            _error = Format.Problem(_text, e);
        }
        finally
        {
            _resetting = false;
            ChangedAll();
        }
    }
}

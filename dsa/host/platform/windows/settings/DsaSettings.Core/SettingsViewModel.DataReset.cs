namespace Dsa.Settings.Core;

/// <summary>Resetting Android's data.</summary>
public sealed partial class SettingsViewModel
{
    private bool _resetting;
    private bool _resetDone;

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

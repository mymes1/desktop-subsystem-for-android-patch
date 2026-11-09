namespace Dsa.Settings.Core;

/// <summary>The Updates page's switch that lets Android restart by itself into a downloaded
/// Android image: at once with no app window open, otherwise after asking or once every
/// window is closed.</summary>
public sealed partial class SettingsViewModel
{
    /// <summary>Whether Android restarts by itself into a downloaded image. On by
    /// default; off, the image is used the next time the user starts Android.</summary>
    public bool ImageAutoRestart => _settings?.ImageAutoRestart ?? true;

    public Task SetImageAutoRestartAsync(bool on) =>
        on == ImageAutoRestart && Loaded
            ? Task.CompletedTask
            : ChangeAsync(() => _service.SetImageAutoRestartAsync(on));
}

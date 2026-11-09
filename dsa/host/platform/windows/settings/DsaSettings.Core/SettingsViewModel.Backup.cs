using System.Globalization;

namespace Dsa.Settings.Core;

/// <summary>What the backup section is doing now.</summary>
public enum BackupActivity
{
    None,
    BackingUp,
    Restoring,
    ShowingKey,
    UsingKey,
}

/// <summary>The backup section of the Advanced page: where the backup of Android's app data
/// stands, backing up and restoring now, and the recovery key that opens the backup folder on
/// another computer. Every action goes through <c>dsa backup</c>.</summary>
public sealed partial class SettingsViewModel
{
    private BackupInfo? _backup;
    private bool _backupLoaded;
    private BackupActivity _backupActivity;
    private string? _backupError;
    private string? _backupResult;
    private string? _recoveryKey;

    public BackupInfo? Backup => _backup;

    /// <summary>Whether the service answered for the backup at all. A service from before
    /// backups, or one that cannot reach the user's backup, leaves the section saying why.</summary>
    public bool HasBackup => _backup is not null;

    /// <summary>Whether backups can be made and restored now: the folder and its key are there.</summary>
    public bool BackupAvailable => _backup?.Available ?? false;

    /// <summary>Why backups cannot be used, when they cannot.</summary>
    public string BackupProblem => _backup switch
    {
        null when _backupLoaded => _text.Get("Backup_NoService"),
        { Available: false, Problem.Length: > 0 } backup => backup.Problem,
        { Available: false } => _text.Get("Backup_Unavailable"),
        _ => "",
    };

    public bool HasBackupProblem => BackupProblem.Length > 0;

    /// <summary>The folder the backup is kept in, in the user's profile.</summary>
    public string BackupFolder => _backup is { Folder.Length: > 0 } backup
        ? string.Format(CultureInfo.CurrentCulture, _text.Get("Backup_Folder"), backup.Folder)
        : "";

    public bool HasBackupFolder => BackupFolder.Length > 0;

    /// <summary>When the last backup was made, in the user's time zone and format.</summary>
    public string BackupLast => string.Format(CultureInfo.CurrentCulture, _text.Get("Backup_Last"),
        When(_backup?.LastBackupUnixMs ?? 0));

    /// <summary>How many apps the backup holds, and how large it is.</summary>
    public string BackupApps => string.Format(CultureInfo.CurrentCulture, _text.Get("Backup_Apps"),
        _backup?.Packages ?? 0, Format.Size((_backup?.Size ?? 0) / 1024));

    /// <summary>What went wrong the last time a backup ran, as the service recorded it.</summary>
    public string BackupLastError => _backup is { LastError.Length: > 0 } backup
        ? string.Format(CultureInfo.CurrentCulture, _text.Get("Backup_LastError"), backup.LastError)
        : "";

    public bool HasBackupLastError => BackupLastError.Length > 0;

    public BackupActivity BackupActivity => _backupActivity;

    public bool IsBackupWorking => _backupActivity != BackupActivity.None;

    /// <summary>A backup or restore is under way; each takes minutes with many apps.</summary>
    public bool IsBackingUpOrRestoring => _backupActivity is BackupActivity.BackingUp or BackupActivity.Restoring;

    public bool CanBackUpNow => !_busy && !IsBackupWorking && BackupAvailable;

    /// <summary>Restoring is offered once the backup holds an app.</summary>
    public bool CanRestoreBackup => CanBackUpNow && (_backup?.Packages ?? 0) > 0;

    public bool CanShowRecoveryKey => !_busy && !IsBackupWorking && BackupAvailable;

    /// <summary>A recovery key can be brought whenever the service keeps backups, also while
    /// the backup cannot be opened: bringing the key is what opens it.</summary>
    public bool CanUseRecoveryKey => !_busy && !IsBackupWorking && HasBackup;

    /// <summary>What the last action in the section failed with.</summary>
    public string BackupError => _backupError ?? "";

    public bool HasBackupError => _backupError is not null;

    /// <summary>What the last backup, restore or recovery key did.</summary>
    public string BackupResult => _backupResult ?? "";

    public bool HasBackupResult => _backupResult is not null;

    /// <summary>The recovery key, once the user asked to see it.</summary>
    public string RecoveryKey => _recoveryKey ?? "";

    public bool HasRecoveryKey => _recoveryKey is not null;

    /// <summary>The button that shows the key is there while the key is not shown.</summary>
    public bool ShowsRecoveryKeyButton => !HasRecoveryKey;

    /// <summary>Tells the user to write the key down and keep it apart from this computer.</summary>
    public string RecoveryKeyAdvice => _text.Get("Backup_KeyAdvice");

    /// <summary>A time in Unix milliseconds in <paramref name="zone"/>, in the format of
    /// <paramref name="culture"/>; 0 or less is never.</summary>
    public static string BackupTime(long unixMs, TimeZoneInfo zone, CultureInfo culture, string never) =>
        unixMs <= 0
            ? never
            : TimeZoneInfo.ConvertTime(DateTimeOffset.FromUnixTimeMilliseconds(unixMs), zone)
                .ToString("g", culture);

    private string When(long unixMs) =>
        BackupTime(unixMs, TimeZoneInfo.Local, CultureInfo.CurrentCulture, _text.Get("Backup_Never"));

    /// <summary>Reads where the backup stands. A service that cannot say leaves the section
    /// saying backups are not there.</summary>
    public async Task LoadBackupAsync()
    {
        try
        {
            _backup = await _service.BackupAsync().ConfigureAwait(true);
        }
        catch (DsaException)
        {
            _backup = null;
        }
        _backupLoaded = true;
        ChangedAll();
    }

    /// <summary>Backs up every app's data now.</summary>
    public Task BackUpNowAsync() => RunBackupAsync(BackupActivity.BackingUp, async () =>
    {
        var report = await _service.BackUpNowAsync().ConfigureAwait(true);
        _backup = report;
        _backupResult = report.Failed > 0
            ? string.Format(CultureInfo.CurrentCulture, _text.Get("Backup_DoneSome"), report.Succeeded, report.Failed)
            : string.Format(CultureInfo.CurrentCulture, _text.Get("Backup_Done"), report.Succeeded);
    });

    /// <summary>Restores every app in the backup. The page asks the user first, with
    /// <see cref="RestoreWarning"/>; this does not ask again.</summary>
    public Task RestoreBackupAsync() => RunBackupAsync(BackupActivity.Restoring, async () =>
    {
        var report = await _service.RestoreBackupAsync().ConfigureAwait(true);
        _backup = report;
        _backupResult = report.Failed > 0
            ? string.Format(CultureInfo.CurrentCulture, _text.Get("Backup_RestoredSome"), report.Succeeded,
                report.Installed, report.Failed)
            : string.Format(CultureInfo.CurrentCulture, _text.Get("Backup_Restored"), report.Succeeded,
                report.Installed);
    });

    /// <summary>What the confirmation of a restore says: each app's data is replaced by the
    /// backup's.</summary>
    public string RestoreWarning => _text.Get("Backup_RestoreWarning");

    /// <summary>Shows the recovery key, with the advice to write it down.</summary>
    public Task ShowRecoveryKeyAsync() => RunBackupAsync(BackupActivity.ShowingKey, async () =>
    {
        var key = await _service.BackupRecoveryKeyAsync().ConfigureAwait(true);
        if (key.Length == 0)
        {
            throw new DsaException(Problem.Unreadable, "dsa printed no recovery key");
        }
        _recoveryKey = key;
    }, clearsResult: false);

    /// <summary>Hides the recovery key again.</summary>
    public void HideRecoveryKey()
    {
        _recoveryKey = null;
        ChangedAll();
    }

    /// <summary>Keeps the key <paramref name="recoveryKey"/> carries as this user's backup key.
    /// An empty entry is not sent.</summary>
    public Task UseRecoveryKeyAsync(string recoveryKey)
    {
        var key = recoveryKey.Trim();
        if (key.Length == 0)
        {
            _backupError = _text.Get("Backup_KeyEmpty");
            ChangedAll();
            return Task.CompletedTask;
        }
        return RunBackupAsync(BackupActivity.UsingKey, async () =>
        {
            _backup = await _service.UseBackupRecoveryKeyAsync(key).ConfigureAwait(true);
            _recoveryKey = null;
            _backupResult = _text.Get("Backup_KeyUsed");
        });
    }

    private async Task RunBackupAsync(BackupActivity activity, Func<Task> work, bool clearsResult = true)
    {
        _backupActivity = activity;
        _backupError = null;
        if (clearsResult)
        {
            _backupResult = null;
        }
        ChangedAll();
        try
        {
            await work().ConfigureAwait(true);
        }
        catch (Exception e)
        {
            _backupError = Format.Problem(_text, e);
        }
        finally
        {
            _backupActivity = BackupActivity.None;
            ChangedAll();
        }
    }
}

using System.Globalization;

namespace Dsa.Settings.Core;

/// <summary>An update of DSA itself while it runs: what it is doing, the version, and that the
/// user should leave DSA alone until it is done. The service reports the phase in its status;
/// while setup replaces the service there is no service to ask, and that gap reads as DSA
/// restarting, not as an error. Once the new service answers, the page says which version
/// runs.</summary>
public sealed class ProgramUpdateViewModel : ObservableObject
{
    /// <summary>How long the service may stay away during an update before the page stops
    /// waiting for it and says so.</summary>
    public static readonly TimeSpan RestartLimit = TimeSpan.FromMinutes(10);

    private readonly DsaService _service;
    private readonly ITextSource _text;
    private readonly Func<DateTimeOffset> _now;
    private ProgramUpdatePhase _phase = ProgramUpdatePhase.None;
    private string _version = "";
    /// <summary>Set while the request to install now waits for its answer.</summary>
    private bool _asking;
    /// <summary>When the service went away during the update, or null while it answers.</summary>
    private DateTimeOffset? _awaySince;
    /// <summary>The service stayed away longer than <see cref="RestartLimit"/>.</summary>
    private bool _notBack;
    private bool _polling;
    private string? _error;

    public ProgramUpdateViewModel(DsaService service, ITextSource text, Func<DateTimeOffset>? now = null)
    {
        _service = service;
        _text = text;
        _now = now ?? (() => DateTimeOffset.UtcNow);
    }

    /// <summary>Raised once the service answers again after it went away during an update, so
    /// the pages read the new version's settings.</summary>
    public event EventHandler? CameBack;

    public ProgramUpdatePhase Phase => _phase;
    /// <summary>The version the update installs, or the one installed or that failed.</summary>
    public string Version => _version;

    /// <summary>Whether an update is under way, so the page shows its progress and the user
    /// should not close DSA or start Android apps.</summary>
    public bool InProgress => _phase is ProgramUpdatePhase.Starting or ProgramUpdatePhase.Downloading
        or ProgramUpdatePhase.StoppingAndroid or ProgramUpdatePhase.Installing or ProgramUpdatePhase.Restarting;

    /// <summary>Whether the install-now button may start an update: not while one runs.</summary>
    public bool CanStartUpdate => !InProgress;

    /// <summary>Whether the service is away while setup replaces it.</summary>
    public bool IsRestarting => _phase == ProgramUpdatePhase.Restarting;
    public bool IsApplied => _phase == ProgramUpdatePhase.Applied;
    public bool IsFailed => _phase == ProgramUpdatePhase.Failed;

    /// <summary>What the update is doing now, in the user's language.</summary>
    public string PhaseText => _phase switch
    {
        ProgramUpdatePhase.Starting => _text.Get("ProgramUpdate_Starting"),
        ProgramUpdatePhase.Downloading => Fill("ProgramUpdate_Downloading"),
        ProgramUpdatePhase.StoppingAndroid => Fill("ProgramUpdate_StoppingAndroid"),
        ProgramUpdatePhase.Installing => Fill("ProgramUpdate_Installing"),
        ProgramUpdatePhase.Restarting => _text.Get("ProgramUpdate_Restarting"),
        _ => "",
    };

    public bool HasVersion => InProgress && _version.Length > 0;
    public string VersionText => HasVersion ? Fill("ProgramUpdate_Version") : "";

    /// <summary>The line that asks the user to leave DSA alone while the update runs.</summary>
    public string KeepOpenText => _text.Get("ProgramUpdate_KeepOpen");

    public string AppliedText => IsApplied ? Fill("ProgramUpdate_Applied") : "";

    public string FailedText => !IsFailed ? "" : _notBack ? Fill("ProgramUpdate_NotBack") : Fill("ProgramUpdate_Failed");

    /// <summary>Why the request to install now was refused, in the user's language.</summary>
    public string Error => _error ?? "";
    public bool HasError => _error is not null;

    private string Fill(string key) => string.Format(CultureInfo.CurrentCulture, _text.Get(key), _version);

    private void Set(ProgramUpdatePhase phase, string? version = null)
    {
        _phase = phase;
        if (!string.IsNullOrEmpty(version))
        {
            _version = version;
        }
        ChangedAll();
    }

    /// <summary>Asks the service to install the newest version now and shows the update from
    /// the moment it is asked. Returns the service's answer, or null when it was refused, in
    /// which case <see cref="Error"/> says why. An answer without a version means DSA is up to
    /// date.</summary>
    public async Task<ProgramUpdateAnswer?> UpdateNowAsync()
    {
        if (InProgress)
        {
            return null;
        }
        _error = null;
        _notBack = false;
        _awaySince = null;
        _version = "";
        _asking = true;
        Set(ProgramUpdatePhase.Starting);
        try
        {
            var answer = await _service.UpdateNowAsync().ConfigureAwait(true);
            _asking = false;
            if (answer.Installing is not { } version)
            {
                Set(ProgramUpdatePhase.None);
                return answer;
            }
            // A poll may already have seen a later phase, or the service go away.
            if (_phase is ProgramUpdatePhase.Starting or ProgramUpdatePhase.Downloading
                or ProgramUpdatePhase.StoppingAndroid)
            {
                Set(ProgramUpdatePhase.Installing, version);
            }
            else
            {
                Set(_phase, version);
            }
            return answer;
        }
        catch (Exception e)
        {
            _asking = false;
            // Setup may stop the service before its answer arrives: once setup runs, a lost
            // answer is the restart, not a refusal.
            if (_phase is ProgramUpdatePhase.Installing or ProgramUpdatePhase.Restarting)
            {
                GoneAway();
                return null;
            }
            _error = Format.Problem(_text, e);
            Set(ProgramUpdatePhase.None);
            return null;
        }
    }

    /// <summary>Reads where the update stands from the running service, as a timer does while
    /// the page is shown. Never starts the service: while setup has stopped it, starting it
    /// again would run the old version under setup.</summary>
    public async Task PollAsync()
    {
        if (_polling)
        {
            return;
        }
        _polling = true;
        try
        {
            StatusInfo status;
            try
            {
                status = await _service.RunningStatusAsync().ConfigureAwait(true);
            }
            catch (Exception)
            {
                // A service that is away matters only while an update runs; otherwise the
                // other pages say it cannot be reached.
                if (InProgress)
                {
                    GoneAway();
                }
                return;
            }
            Take(status.ProgramUpdate);
        }
        finally
        {
            _polling = false;
        }
    }

    private void GoneAway()
    {
        _awaySince ??= _now();
        if (_now() - _awaySince.Value > RestartLimit)
        {
            _notBack = true;
            Set(ProgramUpdatePhase.Failed);
            return;
        }
        Set(ProgramUpdatePhase.Restarting);
    }

    /// <summary>Shows what the service said about the update.</summary>
    private void Take(ProgramUpdateProgress? reported)
    {
        var cameBack = _awaySince is not null;
        _awaySince = null;
        if (cameBack)
        {
            _notBack = false;
            switch (reported?.Phase)
            {
                // The service that setup started says how it went.
                case ProgramUpdatePhase.Applied or ProgramUpdatePhase.Failed:
                    Set(reported.Phase, reported.Version);
                    break;
                // A service started while setup still runs waits for it.
                case { } phase when phase != ProgramUpdatePhase.None:
                    Set(phase, reported!.Version);
                    break;
                // A new service with nothing to report runs the version that was installed.
                default:
                    Set(ProgramUpdatePhase.Applied);
                    break;
            }
            CameBack?.Invoke(this, EventArgs.Empty);
            return;
        }
        if (_asking)
        {
            // Until the request is answered, only a phase of this update counts: the result
            // of an earlier one, or nothing yet, leaves the page saying it is getting ready.
            if (reported is { Phase: ProgramUpdatePhase.Downloading or ProgramUpdatePhase.StoppingAndroid
                or ProgramUpdatePhase.Installing or ProgramUpdatePhase.Restarting })
            {
                Set(reported.Phase, reported.Version);
            }
            return;
        }
        if (reported is null)
        {
            // Nothing is under way. An update the page followed keeps saying how it ended.
            if (InProgress)
            {
                Set(ProgramUpdatePhase.None);
            }
            return;
        }
        Set(reported.Phase, reported.Version);
    }
}

using System.Globalization;
using System.Xml.Linq;
using Xunit;

namespace Dsa.Settings.Core.Tests;

/// <summary>Answers each dsa command line with the answers the test queued for it, in order;
/// the last one stays. An answer can wait until the test lets it go.</summary>
internal sealed class SequenceCli : IDsaCli
{
    private readonly Dictionary<string, Queue<Func<Task<CliResult>>>> _answers = new();
    public List<string> Calls { get; } = new();

    public SequenceCli Then(string line, string json) => Then(line, () => Task.FromResult(new CliResult(0, json, "")));

    public SequenceCli ThenFail(string line, string error) =>
        Then(line, () => Task.FromResult(new CliResult(1, "", error)));

    public SequenceCli Then(string line, Func<Task<CliResult>> answer)
    {
        if (!_answers.TryGetValue(line, out var queue))
        {
            _answers[line] = queue = new Queue<Func<Task<CliResult>>>();
        }
        queue.Enqueue(answer);
        return this;
    }

    public Task<CliResult> RunAsync(IReadOnlyList<string> args, CancellationToken cancel = default)
    {
        var line = string.Join(" ", args);
        Calls.Add(line);
        if (!_answers.TryGetValue(line, out var queue) || queue.Count == 0)
        {
            return Task.FromResult(new CliResult(2, "", $"dsa: the test has no answer for {line}"));
        }
        return queue.Count > 1 ? queue.Dequeue()() : queue.Peek()();
    }
}

/// <summary>Gives each text as its key, with the version after it where the text takes one.</summary>
internal sealed class KeyAndVersionText : ITextSource
{
    public string Get(string key) => key + " {0}";
}

public class ProgramUpdateViewModelTests
{
    private const string Status = "status --no-start --json";
    private const string Update = "update --json";
    private const string Unreachable =
        "dsa: cannot reach the DSA service (no such pipe); check that it is installed and running";

    public ProgramUpdateViewModelTests()
    {
        CultureInfo.CurrentCulture = CultureInfo.InvariantCulture;
    }

    private static string StatusWith(string? phase, string version = "0.1.11") =>
        "{\"state\":\"Stopped\",\"last_reason\":null,\"update_notice\":null,\"problem\":null,\"program_update\":" +
        (phase is null ? "null" : $"{{\"phase\":\"{phase}\",\"version\":\"{version}\",\"in_progress\":true}}") + "}";

    private const string Installing =
        "{\"installing\":\"0.1.11\",\"detail\":\"installing DSA 0.1.11; every Android was stopped\",\"status\":null}";

    private static ProgramUpdateViewModel Model(IDsaCli cli, Func<DateTimeOffset>? now = null) =>
        new(new DsaService(cli), new KeyAndVersionText(), now);

    /// <summary>An answer to the update request that waits until <paramref name="release"/>
    /// completes.</summary>
    private static Func<Task<CliResult>> Held(TaskCompletionSource release, CliResult result) => async () =>
    {
        await release.Task;
        return result;
    };

    [Fact]
    public async Task Fr15_update_now_shows_progress_at_once_and_turns_the_button_off()
    {
        var release = new TaskCompletionSource();
        var cli = new SequenceCli()
            .Then(Update, Held(release, new CliResult(0, Installing, "")))
            .Then(Status, StatusWith(null))
            .Then(Status, StatusWith("failed", "0.1.10"));
        var vm = Model(cli);
        Assert.True(vm.CanStartUpdate);

        var asking = vm.UpdateNowAsync();
        Assert.Equal(ProgramUpdatePhase.Starting, vm.Phase);
        Assert.True(vm.InProgress);
        Assert.False(vm.CanStartUpdate);
        Assert.Equal("ProgramUpdate_Starting {0}", vm.PhaseText);
        Assert.Equal("ProgramUpdate_KeepOpen {0}", vm.KeepOpenText);
        Assert.False(vm.HasVersion, "the version is not known yet");

        // Before the service reports this update, nothing else it says moves the page: not
        // nothing, and not how an earlier update ended.
        await vm.PollAsync();
        Assert.Equal(ProgramUpdatePhase.Starting, vm.Phase);
        await vm.PollAsync();
        Assert.Equal(ProgramUpdatePhase.Starting, vm.Phase);

        release.SetResult();
        var answer = await asking;
        Assert.Equal("0.1.11", answer?.Installing);
        Assert.Equal(ProgramUpdatePhase.Installing, vm.Phase);
        Assert.Equal("ProgramUpdate_Installing 0.1.11", vm.PhaseText);
        Assert.False(vm.CanStartUpdate);
    }

    [Theory]
    [InlineData("downloading", ProgramUpdatePhase.Downloading, "ProgramUpdate_Downloading 0.1.11")]
    [InlineData("stopping_android", ProgramUpdatePhase.StoppingAndroid, "ProgramUpdate_StoppingAndroid 0.1.11")]
    [InlineData("installing", ProgramUpdatePhase.Installing, "ProgramUpdate_Installing 0.1.11")]
    [InlineData("restarting", ProgramUpdatePhase.Restarting, "ProgramUpdate_Restarting {0}")]
    public async Task Fr15_each_phase_the_service_reports_is_shown_with_its_version(
        string name, ProgramUpdatePhase phase, string said)
    {
        var release = new TaskCompletionSource();
        var cli = new SequenceCli()
            .Then(Update, Held(release, new CliResult(0, Installing, "")))
            .Then(Status, StatusWith(name));
        var vm = Model(cli);
        var asking = vm.UpdateNowAsync();
        await vm.PollAsync();
        Assert.Equal(phase, vm.Phase);
        Assert.Equal(said, vm.PhaseText);
        Assert.Equal("0.1.11", vm.Version);
        Assert.True(vm.HasVersion);
        Assert.Equal("ProgramUpdate_Version 0.1.11", vm.VersionText);
        Assert.True(vm.InProgress);
        Assert.False(vm.CanStartUpdate);
        Assert.False(vm.HasError);
        release.SetResult();
        await asking;
        Assert.True(vm.InProgress, "the update is still under way once its request is answered");
    }

    [Fact]
    public async Task Fr15_an_update_started_elsewhere_is_shown_too()
    {
        var cli = new SequenceCli().Then(Status, StatusWith("stopping_android"));
        var vm = Model(cli);
        await vm.PollAsync();
        Assert.Equal(ProgramUpdatePhase.StoppingAndroid, vm.Phase);
        Assert.False(vm.CanStartUpdate);
    }

    [Fact]
    public async Task Fr15_the_restart_gap_reads_as_restarting_and_the_new_version_is_shown_once_it_answers()
    {
        var cli = new SequenceCli()
            .Then(Update, Installing)
            .ThenFail(Status, Unreachable)
            .ThenFail(Status, "dsa: the pipe was closed")
            .Then(Status, StatusWith("applied"));
        var vm = Model(cli);
        var cameBack = 0;
        vm.CameBack += (_, _) => cameBack++;
        await vm.UpdateNowAsync();
        Assert.Equal(ProgramUpdatePhase.Installing, vm.Phase);

        await vm.PollAsync();
        Assert.Equal(ProgramUpdatePhase.Restarting, vm.Phase);
        Assert.True(vm.IsRestarting);
        Assert.Equal("ProgramUpdate_Restarting {0}", vm.PhaseText);
        Assert.False(vm.HasError, "the service being away is the restart, not an error");
        Assert.True(vm.InProgress);
        Assert.False(vm.CanStartUpdate);
        await vm.PollAsync();
        Assert.Equal(ProgramUpdatePhase.Restarting, vm.Phase);
        Assert.Equal(0, cameBack);

        await vm.PollAsync();
        Assert.Equal(ProgramUpdatePhase.Applied, vm.Phase);
        Assert.True(vm.IsApplied);
        Assert.Equal("ProgramUpdate_Applied 0.1.11", vm.AppliedText);
        Assert.False(vm.InProgress);
        Assert.True(vm.CanStartUpdate);
        Assert.Equal(1, cameBack);
        Assert.DoesNotContain("status --json", cli.Calls);
    }

    [Fact]
    public async Task Fr15_a_new_service_with_nothing_to_report_runs_the_version_installed()
    {
        var cli = new SequenceCli()
            .Then(Update, Installing)
            .ThenFail(Status, Unreachable)
            .Then(Status, StatusWith(null));
        var vm = Model(cli);
        await vm.UpdateNowAsync();
        await vm.PollAsync();
        await vm.PollAsync();
        Assert.Equal(ProgramUpdatePhase.Applied, vm.Phase);
        Assert.Equal("ProgramUpdate_Applied 0.1.11", vm.AppliedText);
    }

    [Fact]
    public async Task Fr15_a_service_started_while_setup_runs_keeps_the_update_shown()
    {
        var cli = new SequenceCli()
            .Then(Update, Installing)
            .ThenFail(Status, Unreachable)
            .Then(Status, StatusWith("installing"))
            .ThenFail(Status, Unreachable);
        var vm = Model(cli);
        await vm.UpdateNowAsync();
        await vm.PollAsync();
        await vm.PollAsync();
        Assert.Equal(ProgramUpdatePhase.Installing, vm.Phase);
        await vm.PollAsync();
        Assert.Equal(ProgramUpdatePhase.Restarting, vm.Phase);
    }

    [Fact]
    public async Task Fr15_an_answer_lost_to_the_restart_is_no_failure()
    {
        var release = new TaskCompletionSource();
        var cli = new SequenceCli()
            .Then(Update, Held(release, new CliResult(1, "", "dsa: the service closed the connection")))
            .Then(Status, StatusWith("installing"));
        var vm = Model(cli);
        var asking = vm.UpdateNowAsync();
        await vm.PollAsync();
        Assert.Equal(ProgramUpdatePhase.Installing, vm.Phase);
        release.SetResult();
        Assert.Null(await asking);
        Assert.Equal(ProgramUpdatePhase.Restarting, vm.Phase);
        Assert.False(vm.HasError);
    }

    [Fact]
    public async Task Fr15_a_refused_update_says_why_and_allows_another()
    {
        var cli = new SequenceCli()
            .ThenFail(Update, "dsa: DSA was not updated: these users' Android did not stop in time: alice (Stopping)");
        var vm = Model(cli);
        Assert.Null(await vm.UpdateNowAsync());
        Assert.Equal(ProgramUpdatePhase.None, vm.Phase);
        Assert.True(vm.HasError);
        Assert.Contains("did not stop in time", vm.Error);
        Assert.False(vm.InProgress);
        Assert.True(vm.CanStartUpdate);
    }

    [Fact]
    public async Task Fr15_nothing_newer_shows_no_progress_and_gives_the_services_words()
    {
        var cli = new SequenceCli()
            .Then(Update, "{\"installing\":null,\"detail\":\"DSA 0.1.11 is up to date\",\"status\":null}");
        var vm = Model(cli);
        var answer = await vm.UpdateNowAsync();
        Assert.Null(answer?.Installing);
        Assert.Equal("DSA 0.1.11 is up to date", answer?.Detail);
        Assert.Equal(ProgramUpdatePhase.None, vm.Phase);
        Assert.False(vm.InProgress);
        Assert.False(vm.HasError);
    }

    [Fact]
    public async Task Fr15_a_failed_update_says_so_and_allows_another()
    {
        var cli = new SequenceCli()
            .Then(Update, Installing)
            .Then(Status, StatusWith("failed"));
        var vm = Model(cli);
        await vm.UpdateNowAsync();
        await vm.PollAsync();
        Assert.Equal(ProgramUpdatePhase.Failed, vm.Phase);
        Assert.True(vm.IsFailed);
        Assert.Equal("ProgramUpdate_Failed 0.1.11", vm.FailedText);
        Assert.True(vm.CanStartUpdate);
    }

    [Fact]
    public async Task Fr15_a_service_away_longer_than_the_limit_is_reported_and_still_awaited()
    {
        var now = DateTimeOffset.Parse("2026-11-10T12:00:00Z", CultureInfo.InvariantCulture);
        var cli = new SequenceCli()
            .Then(Update, Installing)
            .ThenFail(Status, Unreachable)
            .ThenFail(Status, Unreachable)
            .Then(Status, StatusWith("applied"));
        var vm = Model(cli, () => now);
        await vm.UpdateNowAsync();
        await vm.PollAsync();
        Assert.Equal(ProgramUpdatePhase.Restarting, vm.Phase);
        now += ProgramUpdateViewModel.RestartLimit + TimeSpan.FromSeconds(1);
        await vm.PollAsync();
        Assert.Equal(ProgramUpdatePhase.Failed, vm.Phase);
        Assert.Equal("ProgramUpdate_NotBack 0.1.11", vm.FailedText);
        // The service that comes back late still says how it went.
        await vm.PollAsync();
        Assert.Equal(ProgramUpdatePhase.Applied, vm.Phase);
    }

    [Fact]
    public async Task Fr15_a_service_that_is_not_running_without_an_update_shows_nothing()
    {
        var cli = new SequenceCli().ThenFail(Status, Unreachable);
        var vm = Model(cli);
        await vm.PollAsync();
        Assert.Equal(ProgramUpdatePhase.None, vm.Phase);
        Assert.False(vm.InProgress);
        Assert.False(vm.HasError);
        Assert.Equal(new[] { Status }, cli.Calls);
    }

    [Fact]
    public async Task Fr15_the_general_page_never_starts_the_service_while_an_update_runs()
    {
        var cli = new SequenceCli()
            .Then(Update, Installing)
            .ThenFail(Status, Unreachable)
            .Then("status --json", StatusWith(null));
        var update = Model(cli);
        var general = new GeneralViewModel(new DsaService(cli), new KeyText()) { Updating = () => update.InProgress };
        await general.PollAsync();
        Assert.Contains("status --json", cli.Calls);

        await update.UpdateNowAsync();
        cli.Calls.Clear();
        await general.PollAsync();
        Assert.Equal(new[] { Status }, cli.Calls);
    }

    [Theory]
    [InlineData(null, ProgramUpdatePhase.None)]
    [InlineData("downloading", ProgramUpdatePhase.Downloading)]
    [InlineData("stopping_android", ProgramUpdatePhase.StoppingAndroid)]
    [InlineData("installing", ProgramUpdatePhase.Installing)]
    [InlineData("restarting", ProgramUpdatePhase.Restarting)]
    [InlineData("applied", ProgramUpdatePhase.Applied)]
    [InlineData("failed", ProgramUpdatePhase.Failed)]
    [InlineData("something newer", ProgramUpdatePhase.None)]
    public void Fr15_each_phase_name_is_read(string? name, ProgramUpdatePhase phase) =>
        Assert.Equal(phase, DsaService.ReadProgramUpdatePhase(name));

    private static XDocument Strings(string file)
    {
        // The tests run from dsa/target/settings/..., outside the source tree, so each folder
        // above is tried both as the settings folder and as the repository's dsa folder.
        string[] relative = { Path.Combine("DsaSettings", file),
            Path.Combine("host", "platform", "windows", "settings", "DsaSettings", file) };
        string? found = null;
        for (var dir = new DirectoryInfo(AppContext.BaseDirectory); dir is not null && found is null; dir = dir.Parent)
        {
            found = relative.Select(r => Path.Combine(dir.FullName, r)).FirstOrDefault(File.Exists);
        }
        Assert.True(found is not null, file + " was not found above " + AppContext.BaseDirectory);
        return XDocument.Load(found!);
    }

    [Theory]
    [InlineData("Strings.resx")]
    [InlineData("Strings.ko.resx")]
    public void Fr15_every_update_progress_text_is_in_both_languages(string file)
    {
        var values = Strings(file).Descendants("data")
            .ToDictionary(d => (string?)d.Attribute("name") ?? "", d => d.Element("value")?.Value ?? "");
        foreach (var name in new[] { "ProgramUpdate_Starting", "ProgramUpdate_Restarting", "ProgramUpdate_KeepOpen",
            "ProgramUpdateCard.Header", "ProgramUpdateDone.Title", "UpdateNowFailed" })
        {
            Assert.True(values.ContainsKey(name), $"{file} has no {name}");
        }
        foreach (var name in new[] { "ProgramUpdate_Downloading", "ProgramUpdate_StoppingAndroid",
            "ProgramUpdate_Installing", "ProgramUpdate_Version", "ProgramUpdate_Applied", "ProgramUpdate_Failed",
            "ProgramUpdate_NotBack" })
        {
            Assert.True(values.ContainsKey(name), $"{file} has no {name}");
            Assert.Contains("{0}", values[name]);
        }
    }
}

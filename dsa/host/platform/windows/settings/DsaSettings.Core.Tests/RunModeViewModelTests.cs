using System.Globalization;
using Xunit;

namespace Dsa.Settings.Core.Tests;

/// <summary>The run mode and the data reset on the Advanced page.</summary>
public class RunModeViewModelTests
{
    private const string Settings =
        "{\"render_gpu\":\"display\",\"freeze\":false,\"program_updates\":true,\"image_updates\":true," +
        "\"program_updates_available\":true,\"image_updates_available\":false,\"version\":\"0.0.9\"," +
        "\"run_mode\":\"as-needed\",\"stop_after_secs\":300,\"keep_for_notifications\":false}";

    public RunModeViewModelTests()
    {
        CultureInfo.CurrentCulture = CultureInfo.InvariantCulture;
    }

    private static FakeCli Cli() => new FakeCli()
        .Answer("settings --json", Settings)
        .Fail("gpus --json", "dsa: no GPU list");

    [Fact]
    public async Task Fr31_run_mode_is_as_needed_after_five_minutes_by_default()
    {
        var vm = new SettingsViewModel(new DsaService(Cli()), new KeyText());
        await vm.LoadAsync();
        Assert.False(vm.Continuous);
        Assert.Equal(300, vm.StopAfterSecs);
        Assert.False(vm.KeepForNotifications);
        Assert.True(vm.CanChangeStopAfter);
        Assert.Equal(300, vm.StopAfterChoices[vm.SelectedStopAfterIndex].Secs);
    }

    [Fact]
    public async Task Fr31_a_service_from_before_the_run_mode_reads_as_needed_after_five_minutes()
    {
        var older = "{\"render_gpu\":\"display\",\"freeze\":false,\"version\":\"0.0.8\"}";
        var vm = new SettingsViewModel(new DsaService(new FakeCli()
            .Answer("settings --json", older)
            .Fail("gpus --json", "dsa: no GPU list")), new KeyText());
        await vm.LoadAsync();
        Assert.False(vm.Continuous);
        Assert.Equal(SettingsInfo.DefaultStopAfterSecs, vm.StopAfterSecs);
    }

    [Fact]
    public async Task Fr31_run_mode_wait_and_notifications_are_changed_through_the_service()
    {
        var continuous = Settings.Replace("\"run_mode\":\"as-needed\"", "\"run_mode\":\"continuous\"");
        var cli = Cli()
            .Answer("settings set run-mode continuous --json", continuous)
            .Answer("settings set run-mode as-needed --json", Settings)
            .Answer("settings set stop-after 900 --json", Settings.Replace("\"stop_after_secs\":300", "\"stop_after_secs\":900"))
            .Answer("settings set keep-for-notifications on --json",
                Settings.Replace("\"keep_for_notifications\":false", "\"keep_for_notifications\":true"));
        var vm = new SettingsViewModel(new DsaService(cli), new KeyText());
        await vm.LoadAsync();

        await vm.SetContinuousAsync(true);
        Assert.True(vm.Continuous);
        Assert.False(vm.CanChangeStopAfter, "the wait only means something as needed");
        await vm.SetContinuousAsync(false);
        Assert.False(vm.Continuous);

        await vm.SetStopAfterAsync(900);
        Assert.Equal(900, vm.StopAfterSecs);
        Assert.Equal(900, vm.StopAfterChoices[vm.SelectedStopAfterIndex].Secs);

        await vm.SetKeepForNotificationsAsync(true);
        Assert.True(vm.KeepForNotifications);
        Assert.All(cli.Calls, call => Assert.EndsWith("--json", call));
    }

    [Fact]
    public async Task Fr31_a_wait_the_service_would_not_keep_is_not_sent()
    {
        var cli = Cli();
        var vm = new SettingsViewModel(new DsaService(cli), new KeyText());
        await vm.LoadAsync();
        await vm.SetStopAfterAsync(5);
        await vm.SetStopAfterAsync(90000);
        Assert.DoesNotContain(cli.Calls, call => call.Contains("stop-after"));
    }

    [Fact]
    public async Task Fr31_a_wait_set_from_the_command_line_shows_as_chosen()
    {
        var vm = new SettingsViewModel(new DsaService(new FakeCli()
            .Answer("settings --json", Settings.Replace("\"stop_after_secs\":300", "\"stop_after_secs\":45"))
            .Fail("gpus --json", "dsa: no GPU list")), new KeyText());
        await vm.LoadAsync();
        Assert.Equal(45, vm.StopAfterChoices[vm.SelectedStopAfterIndex].Secs);
        Assert.Equal("StopAfter_Seconds", vm.StopAfterChoices[vm.SelectedStopAfterIndex].Label);
    }

    [Fact]
    public async Task Fr31_reset_says_what_is_lost_and_goes_through_the_service()
    {
        var cli = Cli().Answer("reset-data --yes --json", "{\"reset\":true}");
        var vm = new SettingsViewModel(new DsaService(cli), new KeyText());
        await vm.LoadAsync();
        Assert.Equal("ResetData_Warning", vm.ResetWarning);
        Assert.True(vm.CanResetData);
        Assert.False(vm.ResetDone);

        await vm.ResetDataAsync();
        Assert.True(vm.ResetDone);
        Assert.False(vm.HasError);
        Assert.Contains("reset-data --yes --json", cli.Calls);
    }

    [Fact]
    public async Task Fr31_a_failed_reset_says_why()
    {
        var cli = Cli().Fail("reset-data --yes --json",
            "dsa: Android did not stop within 120 s (it is Stopping), so its data was left as it is");
        var vm = new SettingsViewModel(new DsaService(cli), new KeyText());
        await vm.LoadAsync();
        await vm.ResetDataAsync();
        Assert.False(vm.ResetDone);
        Assert.True(vm.HasError);
        Assert.Contains("did not stop", vm.Error);
        Assert.True(vm.CanResetData, "the user can try again");
    }
}

using System.Globalization;
using Xunit;

namespace Dsa.Settings.Core.Tests;

/// <summary>The data reset on the Advanced page.</summary>
public class DataResetViewModelTests
{
    private const string Settings =
        "{\"render_gpu\":\"display\",\"freeze\":false,\"program_updates\":true,\"image_updates\":true," +
        "\"program_updates_available\":true,\"image_updates_available\":false,\"version\":\"0.0.9\"}";

    public DataResetViewModelTests()
    {
        CultureInfo.CurrentCulture = CultureInfo.InvariantCulture;
    }

    private static FakeCli Cli() => new FakeCli()
        .Answer("settings --json", Settings)
        .Fail("gpus --json", "dsa: no GPU list");

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

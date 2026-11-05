using System.Globalization;
using Xunit;

namespace Dsa.Settings.Core.Tests;

/// <summary>The Network page: Android's address, DNS servers and mode, and the advanced
/// networking switch.</summary>
public class NetworkViewModelTests
{
    private const string Nat =
        "{\"mode\":\"nat\",\"addresses\":[\"172.28.16.2\",\"fd5a:d5a:0:1::2\"]," +
        "\"dns_servers\":[\"172.28.16.1\"],\"gateway\":\"172.28.16.1\",\"problem\":null}";

    private static string Settings(bool advanced, string network) =>
        "{\"render_gpu\":\"display\",\"freeze\":false,\"program_updates\":true,\"image_updates\":true," +
        "\"program_updates_available\":true,\"image_updates_available\":false,\"version\":\"0.0.9\"," +
        $"\"advanced_networking\":{(advanced ? "true" : "false")},\"network\":{network}}}";

    public NetworkViewModelTests()
    {
        CultureInfo.CurrentCulture = CultureInfo.InvariantCulture;
    }

    private static FakeCli Cli(string settings) => new FakeCli()
        .Answer("settings --json", settings)
        .Fail("arm-translation status --json", "dsa: no ARM translation")
        .Fail("gpus --json", "dsa: no GPU list");

    [Fact]
    public async Task Fr28_the_page_shows_the_nat_address_dns_and_gateway_and_advanced_is_off_by_default()
    {
        var vm = new SettingsViewModel(new DsaService(Cli(Settings(false, Nat))), new KeyText());
        await vm.LoadAsync();
        Assert.False(vm.AdvancedNetworking);
        Assert.Equal(NetworkMode.Nat, vm.Network!.Mode);
        Assert.Equal("Network_ModeNat", vm.NetworkModeText);
        Assert.True(vm.HasNetworkDetails);
        Assert.Equal("172.28.16.2, fd5a:d5a:0:1::2", vm.NetworkAddresses);
        Assert.Equal("172.28.16.1", vm.NetworkDns);
        Assert.Equal("172.28.16.1", vm.NetworkGateway);
        Assert.False(vm.HasNetworkProblem);
        Assert.False(vm.NetworkRestartNeeded);
    }

    [Fact]
    public async Task Fr28_turning_advanced_networking_on_goes_through_the_service_and_waits_for_a_restart()
    {
        var cli = Cli(Settings(false, Nat))
            .Answer("settings set advanced-networking on --json", Settings(true, Nat));
        var vm = new SettingsViewModel(new DsaService(cli), new KeyText());
        await vm.LoadAsync();
        await vm.SetAdvancedNetworkingAsync(true);
        Assert.Contains("settings set advanced-networking on --json", cli.Calls);
        Assert.True(vm.AdvancedNetworking);
        // Android is still behind NAT until it starts again.
        Assert.True(vm.NetworkRestartNeeded);
        // Setting what is already set sends nothing.
        await vm.SetAdvancedNetworkingAsync(true);
        Assert.Single(cli.Calls, call => call.Contains("advanced-networking"));
    }

    [Fact]
    public async Task Fr28_a_problem_with_advanced_networking_is_shown_instead_of_a_restart_notice()
    {
        var fallback = "{\"mode\":\"nat\",\"addresses\":[\"172.28.16.2\"],\"dns_servers\":[]," +
            "\"gateway\":\"172.28.16.1\",\"problem\":\"a Hyper-V virtual switch already uses the adapter\"}";
        var vm = new SettingsViewModel(new DsaService(Cli(Settings(true, fallback))), new KeyText());
        await vm.LoadAsync();
        Assert.True(vm.HasNetworkProblem);
        Assert.Contains("virtual switch", vm.NetworkProblem);
        Assert.False(vm.NetworkRestartNeeded);
        Assert.Equal("Network_None", vm.NetworkDns);
    }

    [Fact]
    public async Task Fr28_a_stopped_android_has_no_network_and_a_refresh_reads_dsa_network()
    {
        var cli = Cli(Settings(false, "{\"mode\":\"off\",\"addresses\":[],\"dns_servers\":[],\"gateway\":\"\"}"))
            .Answer("network --json", Nat);
        var vm = new SettingsViewModel(new DsaService(cli), new KeyText());
        await vm.LoadAsync();
        Assert.Equal("Network_ModeOff", vm.NetworkModeText);
        Assert.False(vm.HasNetworkDetails);
        await vm.RefreshNetworkAsync();
        Assert.Equal("Network_ModeNat", vm.NetworkModeText);
        Assert.Equal("172.28.16.2, fd5a:d5a:0:1::2", vm.NetworkAddresses);
    }

    [Fact]
    public async Task Fr28_a_service_that_does_not_report_the_network_says_so()
    {
        var settings = "{\"render_gpu\":\"display\",\"version\":\"0.0.9\"}";
        var vm = new SettingsViewModel(new DsaService(Cli(settings)), new KeyText());
        await vm.LoadAsync();
        Assert.Null(vm.Network);
        Assert.Equal("Network_ModeUnknown", vm.NetworkModeText);
        Assert.False(vm.AdvancedNetworking);
    }
}

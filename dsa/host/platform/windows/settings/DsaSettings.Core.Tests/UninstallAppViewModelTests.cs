using System.Globalization;
using Xunit;

namespace Dsa.Settings.Core.Tests;

/// <summary>The window Windows' list of installed apps opens to uninstall an Android app.</summary>
public class UninstallAppViewModelTests
{
    private const string Notes =
        "{\"package\":\"com.example.notes\",\"label\":\"Notes\",\"version_name\":\"4.1\",\"version_code\":41," +
        "\"system_app\":false,\"app_bytes\":2097152,\"data_bytes\":1048576,\"cache_bytes\":0}";

    public UninstallAppViewModelTests()
    {
        CultureInfo.CurrentCulture = CultureInfo.InvariantCulture;
    }

    [Fact]
    public async Task Fr23_uninstall_window_names_the_app_and_asks_before_uninstalling()
    {
        var cli = new FakeCli()
            .Answer("apps info com.example.notes --json", Notes)
            .Answer("apps uninstall com.example.notes --json",
                "{\"package\":\"com.example.notes\",\"uninstalled\":true,\"status\":\"uninstalled\",\"message\":\"\"}");
        var vm = new UninstallAppViewModel(cli, new KeyText(), "com.example.notes");
        Assert.Equal(UninstallPage.Reading, vm.Page);
        Assert.Equal("com.example.notes", vm.Title);

        await vm.LoadAsync();
        Assert.Equal(UninstallPage.Confirm, vm.Page);
        Assert.Equal("Notes", vm.Title);
        Assert.Equal("3.0 MB", vm.SizeChip);
        Assert.True(vm.CanUninstall);
        Assert.DoesNotContain(cli.Calls, call => call.StartsWith("apps uninstall", StringComparison.Ordinal));

        await vm.UninstallAsync();
        Assert.Equal(UninstallPage.Done, vm.Page);
        Assert.False(vm.CanUninstall);
        Assert.Contains("apps uninstall com.example.notes --json", cli.Calls);
    }

    [Fact]
    public async Task Fr23_uninstall_window_says_why_android_did_not_uninstall()
    {
        var cli = new FakeCli()
            .Answer("apps info com.example.notes --json", Notes)
            .Fail("apps uninstall com.example.notes --json", "dsa: not uninstalled (blocked): a device policy keeps it");
        var vm = new UninstallAppViewModel(cli, new KeyText(), "com.example.notes");
        await vm.LoadAsync();
        await vm.UninstallAsync();
        Assert.Equal(UninstallPage.Failed, vm.Page);
        Assert.Equal("UninstallWindow_FailedTitle", vm.ResultTitle);
    }

    [Fact]
    public async Task Fr23_uninstall_window_does_not_offer_to_uninstall_a_system_app()
    {
        var cli = new FakeCli()
            .Answer("apps info com.android.settings --json",
                "{\"package\":\"com.android.settings\",\"label\":\"Settings\",\"system_app\":true}");
        var vm = new UninstallAppViewModel(cli, new KeyText(), "com.android.settings");
        await vm.LoadAsync();
        Assert.Equal(UninstallPage.Failed, vm.Page);
        await vm.UninstallAsync();
        Assert.DoesNotContain(cli.Calls, call => call.StartsWith("apps uninstall", StringComparison.Ordinal));
    }

    [Fact]
    public async Task Fr23_uninstall_window_still_offers_the_uninstall_when_android_cannot_describe_the_app()
    {
        var cli = new FakeCli()
            .Fail("apps info com.example.notes --json", "dsa: cannot reach the DSA service")
            .Fail("apps uninstall com.example.notes --json", "dsa: cannot reach the DSA service");
        var vm = new UninstallAppViewModel(cli, new KeyText(), "com.example.notes");
        await vm.LoadAsync();
        Assert.Equal(UninstallPage.Confirm, vm.Page);
        Assert.Equal("com.example.notes", vm.Title);
        await vm.UninstallAsync();
        Assert.Equal(UninstallPage.Failed, vm.Page);
        Assert.Contains("Problem_ServiceUnreachable", vm.ResultText);
    }
}

using System.Globalization;
using Xunit;

namespace Dsa.Settings.Core.Tests;

/// <summary>The window Windows' list of installed apps opens to uninstall an Android app.
/// Windows has already asked, so the window uninstalls at once.</summary>
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
    public async Task Fr23_uninstall_window_uninstalls_at_once_without_asking_again()
    {
        var cli = new FakeCli()
            .Answer("apps info com.example.notes --json", Notes)
            .Answer("apps uninstall com.example.notes --json",
                "{\"package\":\"com.example.notes\",\"uninstalled\":true,\"status\":\"uninstalled\",\"message\":\"\"}");
        var vm = new UninstallAppViewModel(cli, new KeyText(), "com.example.notes");
        Assert.Equal(UninstallPage.Progress, vm.Page);
        Assert.Equal("com.example.notes", vm.Title);

        await vm.RunAsync();
        Assert.Equal("Notes", vm.Title);
        Assert.Equal("UninstallWindow_Version", vm.VersionChip);
        Assert.Equal(UninstallPage.Done, vm.Page);
        Assert.True(vm.ClosesItself);
        Assert.Equal(new[] { "apps info com.example.notes --json", "apps uninstall com.example.notes --json" }, cli.Calls);

        await vm.RunAsync();
        Assert.Equal(2, cli.Calls.Count);
    }

    [Fact]
    public async Task Fr23_uninstall_window_says_why_android_did_not_uninstall_and_stays_open()
    {
        var cli = new FakeCli()
            .Answer("apps info com.example.notes --json", Notes)
            .Fail("apps uninstall com.example.notes --json", "dsa: not uninstalled (blocked): a device policy keeps it");
        var vm = new UninstallAppViewModel(cli, new KeyText(), "com.example.notes");
        await vm.RunAsync();
        Assert.Equal(UninstallPage.Failed, vm.Page);
        Assert.False(vm.ClosesItself);
        Assert.Equal("UninstallWindow_FailedTitle", vm.ResultTitle);
    }

    [Fact]
    public async Task Fr23_uninstall_window_does_not_try_to_uninstall_a_system_app()
    {
        var cli = new FakeCli()
            .Answer("apps info com.android.settings --json",
                "{\"package\":\"com.android.settings\",\"label\":\"Settings\",\"system_app\":true}");
        var vm = new UninstallAppViewModel(cli, new KeyText(), "com.android.settings");
        await vm.RunAsync();
        Assert.Equal(UninstallPage.Failed, vm.Page);
        Assert.DoesNotContain(cli.Calls, call => call.StartsWith("apps uninstall", StringComparison.Ordinal));
    }

    [Fact]
    public async Task Fr23_uninstall_window_still_uninstalls_when_android_cannot_describe_the_app()
    {
        var cli = new FakeCli()
            .Fail("apps info com.example.notes --json", "dsa: cannot reach the DSA service")
            .Fail("apps uninstall com.example.notes --json", "dsa: cannot reach the DSA service");
        var vm = new UninstallAppViewModel(cli, new KeyText(), "com.example.notes");
        await vm.RunAsync();
        Assert.Contains("apps uninstall com.example.notes --json", cli.Calls);
        Assert.Equal(UninstallPage.Failed, vm.Page);
        Assert.Equal("com.example.notes", vm.Title);
        Assert.Contains("Problem_ServiceUnreachable", vm.ResultText);
    }
}

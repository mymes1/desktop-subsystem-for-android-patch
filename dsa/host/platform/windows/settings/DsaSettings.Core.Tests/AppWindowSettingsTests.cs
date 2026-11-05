using System.Globalization;
using Xunit;

namespace Dsa.Settings.Core.Tests;

/// <summary>The window settings of each app on the Apps page: whether its windows may be
/// resized, and the size they open at, kept by the service through dsa app-window.</summary>
public class AppWindowSettingsTests
{
    private const string List =
        "{\"apps\":[" +
        "{\"package\":\"com.example.notes\",\"label\":\"Notes\",\"version_name\":\"4.1\",\"version_code\":41," +
        "\"first_install_time_ms\":0,\"last_update_time_ms\":0,\"system_app\":false," +
        "\"app_bytes\":0,\"data_bytes\":0,\"cache_bytes\":0,\"total_bytes\":0,\"icon_png\":\"\"}," +
        "{\"package\":\"com.example.reader\",\"label\":\"Reader\",\"version_name\":\"1\",\"version_code\":1," +
        "\"first_install_time_ms\":0,\"last_update_time_ms\":0,\"system_app\":false," +
        "\"app_bytes\":0,\"data_bytes\":0,\"cache_bytes\":0,\"total_bytes\":0,\"icon_png\":\"\"}]}";

    private const string Windows =
        "{\"apps\":[{\"package\":\"com.example.reader\",\"fixed_size\":true,\"start_size\":[412,915]}]}";

    public AppWindowSettingsTests()
    {
        CultureInfo.CurrentCulture = CultureInfo.InvariantCulture;
    }

    private static AppsViewModel Make(FakeCli cli) => new(new AppsService(cli), new AppsText());

    [Fact]
    public async Task Fr30_each_app_shows_whether_its_windows_resize_and_the_size_they_open_at()
    {
        var vm = Make(new FakeCli()
            .Answer("apps list --json", List)
            .Answer("app-window --json", Windows));
        await vm.RefreshAsync();
        await vm.LoadWindowSettingsAsync();
        Assert.False(vm.HasError, vm.Error);
        var notes = vm.Rows.Single(r => r.Package == "com.example.notes");
        Assert.True(notes.Resizable);
        Assert.Equal(0, notes.StartSizeIndex);
        var reader = vm.Rows.Single(r => r.Package == "com.example.reader");
        Assert.False(reader.Resizable);
        Assert.Equal(412, vm.StartSizeChoices[reader.StartSizeIndex].Width);
        Assert.Equal(915, vm.StartSizeChoices[reader.StartSizeIndex].Height);
    }

    [Fact]
    public async Task Fr30_the_user_makes_an_apps_windows_fixed_in_size_and_opens_them_as_a_tablet()
    {
        var cli = new FakeCli()
            .Answer("apps list --json", List)
            .Answer("app-window --json", "{\"apps\":[]}")
            .Answer("app-window com.example.notes --fixed-size on --start-size auto --json",
                "{\"apps\":[{\"package\":\"com.example.notes\",\"fixed_size\":true,\"start_size\":null}]}")
            .Answer("app-window com.example.notes --fixed-size on --start-size 1280x800 --json",
                "{\"apps\":[{\"package\":\"com.example.notes\",\"fixed_size\":true,\"start_size\":[1280,800]}]}");
        var vm = Make(cli);
        await vm.RefreshAsync();
        await vm.LoadWindowSettingsAsync();
        await vm.SetResizableAsync("com.example.notes", false);
        Assert.False(vm.HasError, vm.Error);
        Assert.Equal("Apps_WindowChanged", vm.Message);
        Assert.False(vm.Rows.Single(r => r.Package == "com.example.notes").Resizable);

        var tablet = vm.StartSizeChoices.ToList().FindIndex(c => c.Width == 1280 && c.Height == 800);
        await vm.SetStartSizeAsync("com.example.notes", tablet);
        Assert.False(vm.HasError, vm.Error);
        Assert.Equal(tablet, vm.Rows.Single(r => r.Package == "com.example.notes").StartSizeIndex);
        // Choosing the size the app has already sends nothing.
        var calls = cli.Calls.Count;
        await vm.SetStartSizeAsync("com.example.notes", tablet);
        Assert.Equal(calls, cli.Calls.Count);
    }

    [Fact]
    public async Task Fr30_a_service_that_keeps_no_window_settings_leaves_every_app_at_the_defaults()
    {
        var vm = Make(new FakeCli()
            .Answer("apps list --json", List)
            .Fail("app-window --json", "dsa: unexpected \"app-window\""));
        await vm.RefreshAsync();
        await vm.LoadWindowSettingsAsync();
        Assert.False(vm.HasError, vm.Error);
        Assert.All(vm.Rows, row => Assert.True(row.Resizable));
    }

    [Fact]
    public void Fr30_a_start_size_is_passed_to_dsa_as_it_takes_it()
    {
        Assert.Equal("auto", AppWindowSetting.Default("com.example.notes").StartSizeArgument);
        Assert.Equal("412x915", new AppWindowSetting("com.example.notes", false, 412, 915).StartSizeArgument);
    }
}

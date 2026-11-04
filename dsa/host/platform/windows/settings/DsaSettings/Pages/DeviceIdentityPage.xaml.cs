using System.Windows;
using System.Windows.Controls;
using Dsa.Settings.Core;

namespace Dsa.Settings.Pages;

public partial class DeviceIdentityPage : Page
{
    private static DeviceIdentityViewModel ViewModel => App.DeviceIdentity;

    public DeviceIdentityPage()
    {
        InitializeComponent();
        DataContext = ViewModel;
        Loaded += async (_, _) => await ViewModel.LoadAsync();
    }

    private async void OnSave(object sender, RoutedEventArgs e) => await ViewModel.SaveAsync();

    private async void OnReset(object sender, RoutedEventArgs e)
    {
        if (await Shell.Confirm(App.Text.Get("IdentityResetConfirmTitle"), App.Text.Get("IdentityResetConfirmBody"),
                App.Text.Get("IdentityResetButton.Content")))
        {
            await ViewModel.ResetAsync();
        }
    }

    private async void OnRestart(object sender, RoutedEventArgs e) => await ViewModel.RestartAsync();
}

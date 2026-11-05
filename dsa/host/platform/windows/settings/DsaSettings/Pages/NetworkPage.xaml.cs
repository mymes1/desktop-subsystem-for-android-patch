using System.Windows;
using System.Windows.Controls;
using Dsa.Settings.Core;

namespace Dsa.Settings.Pages;

public partial class NetworkPage : Page
{
    private static SettingsViewModel ViewModel => App.Settings;

    public NetworkPage()
    {
        InitializeComponent();
        DataContext = ViewModel;
        Loaded += async (_, _) =>
        {
            await ViewModel.LoadAsync();
            await ViewModel.RefreshNetworkAsync();
        };
    }

    private async void OnRefresh(object sender, RoutedEventArgs e) => await ViewModel.RefreshNetworkAsync();

    private async void OnAdvancedNetworkingToggled(object sender, RoutedEventArgs e) =>
        await ViewModel.SetAdvancedNetworkingAsync(AdvancedNetworkingSwitch.IsChecked == true);
}

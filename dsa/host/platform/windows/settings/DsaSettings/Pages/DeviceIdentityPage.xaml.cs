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

    /// <summary>Copies every specification as text, a row per line, and shows a check mark on
    /// the button for a moment. The clipboard can be held by another program, and then the
    /// button stays as it was.</summary>
    private async void OnCopySpecs(object sender, RoutedEventArgs e)
    {
        try
        {
            System.Windows.Clipboard.SetDataObject(ViewModel.AboutText, copy: true);
        }
        catch (System.Runtime.InteropServices.ExternalException)
        {
            return;
        }
        SpecsCopyButton.Icon = new Wpf.Ui.Controls.SymbolIcon { Symbol = Wpf.Ui.Controls.SymbolRegular.Checkmark24 };
        await System.Threading.Tasks.Task.Delay(1500);
        SpecsCopyButton.Icon = null;
    }
}

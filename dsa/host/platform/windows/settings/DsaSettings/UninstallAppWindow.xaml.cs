using System.ComponentModel;
using System.IO;
using System.Windows;
using System.Windows.Interop;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using System.Windows.Shapes;
using Dsa.Settings.Core;
using Palette = Dsa.Setup.Palette;

namespace Dsa.Settings;

/// <summary>
/// The window Windows' list of installed apps opens to uninstall an Android app. It asks once,
/// then shows the uninstall and how it ended. It follows the theme Windows uses for apps, as the
/// APK install window does.
/// </summary>
public partial class UninstallAppWindow : Window
{
    private const int WmSettingChange = 0x001A;

    private readonly UninstallAppViewModel _vm;
    private bool _dark;

    private UninstallAppWindow(string package)
    {
        _vm = new UninstallAppViewModel(new ProcessDsaCli(ProcessDsaCli.Locate(AppContext.BaseDirectory)), App.Text, package);
        InitializeComponent();
        _dark = Palette.IsDark();
        Palette.Apply(this, _dark);
        if (!Palette.HasMica)
        {
            // Without Mica a full glass frame would show black; a 1 px edge keeps the shadow.
            Chrome.GlassFrameThickness = new Thickness(0, 0, 0, 1);
        }
        Icon = BitmapFrame.Create(new Uri("pack://application:,,,/Assets/logo.ico"));
        Title = App.Text.Get("UninstallWindow_Caption");
        CaptionTitle.Text = Title;
        ReadingText.Text = App.Text.Get("UninstallWindow_Reading");
        ProgressDetail.Text = App.Text.Get("UninstallWindow_BusyDetail");
        CancelButton.Content = App.Text.Get("UninstallWindow_Cancel");
        CloseButton.Content = App.Text.Get("UninstallWindow_Close");
        UninstallButton.Content = App.Text.Get("UninstallWindow_Uninstall");
        System.Windows.Automation.AutomationProperties.SetName(CaptionClose, App.Text.Get("UninstallWindow_Close"));
        ApkInstallWindow.Marquee(ReadingBar);
        ApkInstallWindow.Marquee(ProgressMarquee);
        _vm.PropertyChanged += (_, _) => Render();
        Render();
        SourceInitialized += (_, _) =>
        {
            var handle = new WindowInteropHelper(this).Handle;
            Palette.ApplyBackdrop(handle, _dark);
            HwndSource.FromHwnd(handle)?.AddHook(Hook);
        };
        Loaded += async (_, _) => await _vm.LoadAsync();
    }

    /// <summary>Opens a window that uninstalls <paramref name="package"/>.</summary>
    public static UninstallAppWindow Open(string package)
    {
        var window = new UninstallAppWindow(package);
        window.Show();
        window.Activate();
        return window;
    }

    private IntPtr Hook(IntPtr hwnd, int msg, IntPtr wParam, IntPtr lParam, ref bool handled)
    {
        if (msg == WmSettingChange)
        {
            var dark = Palette.IsDark();
            if (dark != _dark)
            {
                _dark = dark;
                Palette.Apply(this, dark);
            }
        }
        return IntPtr.Zero;
    }

    private static Visibility Visible(bool visible) => visible ? Visibility.Visible : Visibility.Collapsed;

    private void Render()
    {
        var page = _vm.Page;
        HeroTitle.Text = _vm.Title;
        HeroPackage.Text = _vm.Package;
        VersionChip.Text = _vm.VersionChip;
        VersionBorder.Visibility = Visible(_vm.VersionChip.Length > 0);
        SizeChip.Text = _vm.SizeChip;
        SizeBorder.Visibility = Visible(_vm.SizeChip.Length > 0);
        Chips.Visibility = Visible(_vm.VersionChip.Length > 0 || _vm.SizeChip.Length > 0);
        AppIcon.Source = IconImage(_vm.App?.IconPng);

        ReadingPanel.Visibility = Visible(page == UninstallPage.Reading);
        SummaryText.Visibility = Visible(page == UninstallPage.Confirm);
        ProgressPanel.Visibility = Visible(page == UninstallPage.Progress);
        ResultPanel.Visibility = Visible(page is UninstallPage.Done or UninstallPage.Failed);

        SummaryText.Text = _vm.Summary;
        ProgressTitle.Text = _vm.ProgressTitle;

        var good = page == UninstallPage.Done;
        ResultBadge.SetResourceReference(Shape.FillProperty, good ? "Accent" : "Critical");
        ResultGlyph.SetResourceReference(Shape.StrokeProperty, good ? "AccentText" : "CriticalText");
        ResultGlyph.Data = Geometry.Parse(good ? "M3.5,8.5 L6.5,11.5 L12.5,4.5" : "M5.5,5.5 L10.5,10.5 M10.5,5.5 L5.5,10.5");
        ResultTitle.Text = _vm.ResultTitle;
        ResultText.Text = _vm.ResultText;

        UninstallButton.Visibility = Visible(page is UninstallPage.Reading or UninstallPage.Confirm);
        UninstallButton.IsEnabled = _vm.CanUninstall;
        CancelButton.Visibility = Visible(page is UninstallPage.Reading or UninstallPage.Confirm);
        CloseButton.Visibility = Visible(page is UninstallPage.Done or UninstallPage.Failed);
        CaptionClose.IsEnabled = page != UninstallPage.Progress;

        UninstallButton.IsDefault = _vm.CanUninstall;
        CloseButton.IsDefault = CloseButton.Visibility == Visibility.Visible;
        CancelButton.IsCancel = CancelButton.Visibility == Visibility.Visible;
        CloseButton.IsCancel = !CancelButton.IsCancel;
    }

    /// <summary>The app's icon, or DSA's logo without one.</summary>
    private static ImageSource IconImage(byte[]? png)
    {
        if (png is { Length: > 0 })
        {
            try
            {
                var image = new BitmapImage();
                image.BeginInit();
                image.CacheOption = BitmapCacheOption.OnLoad;
                image.StreamSource = new MemoryStream(png);
                image.EndInit();
                image.Freeze();
                return image;
            }
            catch (Exception e) when (e is NotSupportedException or IOException or ArgumentException or FileFormatException)
            {
                // An image Windows cannot decode shows as DSA's logo.
            }
        }
        return new BitmapImage(new Uri("pack://application:,,,/Assets/logo.png"));
    }

    private async void OnUninstall(object sender, RoutedEventArgs e) => await _vm.UninstallAsync();

    private void OnClose(object sender, RoutedEventArgs e) => Close();

    protected override void OnClosing(CancelEventArgs e)
    {
        // Android finishes an uninstall it has begun, so the window stays until it says how it went.
        if (_vm.Page == UninstallPage.Progress)
        {
            e.Cancel = true;
        }
        base.OnClosing(e);
    }
}

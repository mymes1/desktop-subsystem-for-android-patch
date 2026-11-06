using System.ComponentModel;
using System.IO;
using System.Windows;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using Dsa.Settings.Core;
using Microsoft.Win32;
using Wpf.Ui.Appearance;
using Wpf.Ui.Controls;

namespace Dsa.ApkInstaller;

/// <summary>
/// The window an APK opens in: what the file says about the app, then its install. It follows
/// the theme Windows uses for apps, with Mica behind it on Windows 11, and changes with it.
/// </summary>
public partial class InstallerWindow : FluentWindow
{
    private readonly ApkInstallViewModel _vm;
    private bool _closed;

    private InstallerWindow(string path)
    {
        _vm = new ApkInstallViewModel(
            new ProcessDsaRunner(ProcessDsaCli.Locate(AppContext.BaseDirectory)), App.Text, path);
        InitializeComponent();
        Icon = BitmapFrame.Create(new Uri("pack://application:,,,/Assets/apk-installer.ico"));
        Resources["SensitiveText"] = App.Text.Get("ApkWindow_Sensitive");
        Title = App.Text.Get("ApkWindow_Caption");
        TitleBar.Title = Title;
        TrademarkText.Text = App.Text.Get("AboutTrademark.Text");
        ReadingText.Text = App.Text.Get("ApkWindow_Reading");
        DetailsHeader.Text = App.Text.Get("ApkWindow_DetailsHeader");
        LaunchWhenReady.Content = App.Text.Get("ApkWindow_LaunchWhenReady");
        LaunchWhenReady.IsChecked = _vm.LaunchWhenReady;
        LaunchWhenReady.Checked += (_, _) => _vm.LaunchWhenReady = true;
        LaunchWhenReady.Unchecked += (_, _) => _vm.LaunchWhenReady = false;
        CancelButton.Content = App.Text.Get("ApkWindow_Cancel");
        CloseButton.Content = App.Text.Get("ApkWindow_Close");
        OpenButton.Content = App.Text.Get("ApkWindow_Open");
        // The light or the dark theme Windows uses for apps, its accent colour, and their
        // changes while the window is open.
        SystemThemeWatcher.Watch(this, WindowBackdropType.Mica, true);
        _vm.PropertyChanged += (_, _) => Render();
        Render();
        Loaded += async (_, _) =>
        {
            // The watcher follows changes; the theme to start with is the apps' setting, which
            // it does not read itself.
            ApplicationThemeManager.Apply(AppsUseDarkTheme() ? ApplicationTheme.Dark : ApplicationTheme.Light,
                WindowBackdropType.Mica, true);
            await _vm.LoadAsync();
        };
        DragOver += (_, e) =>
        {
            e.Effects = Files(e).Any(InstallViewModel.IsApk) ? DragDropEffects.Copy : DragDropEffects.None;
            e.Handled = true;
        };
        Drop += (_, e) =>
        {
            foreach (var apk in Files(e).Where(InstallViewModel.IsApk))
            {
                Open(apk);
            }
        };
    }

    /// <summary>Opens a window for the APK at <paramref name="path"/>.</summary>
    public static InstallerWindow Open(string path)
    {
        var window = new InstallerWindow(path);
        window.Show();
        window.Activate();
        return window;
    }

    private static bool AppsUseDarkTheme()
    {
        try
        {
            using var key = Registry.CurrentUser.OpenSubKey(@"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
            return key?.GetValue("AppsUseLightTheme") is int light && light == 0;
        }
        catch (Exception e) when (e is System.Security.SecurityException or UnauthorizedAccessException or IOException)
        {
            return false;
        }
    }

    private static IEnumerable<string> Files(DragEventArgs e) =>
        e.Data.GetDataPresent(DataFormats.FileDrop) && e.Data.GetData(DataFormats.FileDrop) is string[] files
            ? files
            : Enumerable.Empty<string>();

    private static Visibility Visible(bool visible) => visible ? Visibility.Visible : Visibility.Collapsed;

    private void Render()
    {
        if (_closed)
        {
            return;
        }
        if (_vm.Launched)
        {
            // The app opened by itself, as "Launch when ready" asked: the installer is done.
            _closed = true;
            Close();
            return;
        }
        var page = _vm.Page;
        var read = _vm.Apk is not null;
        HeroTitle.Text = _vm.Title;
        HeroPackage.Text = _vm.Subtitle;
        HeroPackage.Visibility = Visible(read);
        PublisherText.Text = _vm.PublisherLine;
        PublisherText.Visibility = Visible(read);
        VersionChip.Text = _vm.VersionChip;
        VersionChip.Visibility = Visible(read);
        AppIcon.Source = IconImage(_vm.Apk?.Icon);

        var showDetails = page is ApkInstallPage.Details or ApkInstallPage.Progress;
        SummaryPanel.Visibility = Visible(showDetails);
        SummaryText.Text = _vm.Summary;
        SummaryBadge.Visibility = Visible(_vm.IsBlocked);
        CompatibilityBar.Message = _vm.CompatibilityWarning;
        CompatibilityBar.IsOpen = _vm.CompatibilityWarning.Length > 0;
        CompatibilityBar.Visibility = Visible(_vm.CompatibilityWarning.Length > 0);

        ReadingPanel.Visibility = Visible(page == ApkInstallPage.Reading);
        DetailsPanel.Visibility = Visible(showDetails);
        ResultPanel.Visibility = Visible(page is ApkInstallPage.Done or ApkInstallPage.Failed);

        FactList.ItemsSource = _vm.Facts;
        PermissionsHeader.Text = _vm.PermissionsHeader;
        PermissionList.ItemsSource = _vm.Permissions;

        LaunchWhenReady.Visibility = Visible(page is ApkInstallPage.Reading or ApkInstallPage.Details);
        LaunchWhenReady.IsEnabled = !_vm.IsBlocked;
        ProgressPanel.Visibility = Visible(page == ApkInstallPage.Progress);
        ProgressTitle.Text = _vm.ProgressTitle;
        ProgressDetail.Text = _vm.ProgressDetail;
        Progress.IsIndeterminate = _vm.ProgressPercent is null;
        Progress.Value = _vm.ProgressPercent ?? 0;
        ProgressPercent.Text = _vm.ProgressPercent is { } percent ? percent + "%" : "";

        var good = page == ApkInstallPage.Done;
        ResultGlyph.Symbol = good ? SymbolRegular.CheckmarkCircle24 : SymbolRegular.ErrorCircle24;
        ResultGlyph.SetResourceReference(IconElement.ForegroundProperty,
            good ? "SystemFillColorSuccessBrush" : "SystemFillColorCriticalBrush");
        ResultTitle.Text = _vm.ResultTitle;
        ResultText.Text = _vm.ResultText;

        InstallButton.Content = _vm.InstallButton;
        InstallButton.Visibility = Visible(page is ApkInstallPage.Reading or ApkInstallPage.Details);
        InstallButton.IsEnabled = _vm.CanInstall;
        CancelButton.Visibility = Visible(page is ApkInstallPage.Reading or ApkInstallPage.Details or ApkInstallPage.Progress);
        CancelButton.IsEnabled = page != ApkInstallPage.Progress || _vm.CanCancel;
        OpenButton.Visibility = Visible(_vm.CanOpen);
        CloseButton.Visibility = Visible(page is ApkInstallPage.Done or ApkInstallPage.Failed);

        // Enter presses the screen's main button; Escape its way out.
        InstallButton.IsDefault = _vm.CanInstall;
        OpenButton.IsDefault = _vm.CanOpen;
        CloseButton.IsDefault = page == ApkInstallPage.Failed;
        CancelButton.IsCancel = CancelButton.Visibility == Visibility.Visible;
        CloseButton.IsCancel = !CancelButton.IsCancel;
    }

    /// <summary>The app's icon at the tile's size: its image, or its adaptive icon's layers
    /// masked to a rounded square as a launcher shows them. Without one, the installer's own.</summary>
    private static ImageSource IconImage(ApkIcon? icon)
    {
        try
        {
            if (icon?.Png is { } png)
            {
                return Bitmap(png);
            }
            if (icon is { IsAdaptive: true })
            {
                // Each layer is 108 dp; the middle 72 dp show.
                var full = new Rect(0, 0, 108, 108);
                var group = new DrawingGroup { ClipGeometry = new RectangleGeometry(new Rect(18, 18, 72, 72), 16, 16) };
                foreach (var layer in new[] { icon.Background, icon.Foreground })
                {
                    if (layer?.Color is { } argb)
                    {
                        var color = Color.FromArgb((byte)(argb >> 24), (byte)(argb >> 16), (byte)(argb >> 8), (byte)argb);
                        group.Children.Add(new GeometryDrawing(new SolidColorBrush(color), null, new RectangleGeometry(full)));
                    }
                    else if (layer?.Png is { } image)
                    {
                        group.Children.Add(new ImageDrawing(Bitmap(image), full));
                    }
                    else if (layer == icon.Background)
                    {
                        group.Children.Add(new GeometryDrawing(Brushes.White, null, new RectangleGeometry(full)));
                    }
                }
                var drawn = new DrawingImage(group);
                drawn.Freeze();
                return drawn;
            }
        }
        catch (Exception e) when (e is NotSupportedException or IOException or ArgumentException or FileFormatException)
        {
            // An image Windows cannot decode shows as the installer's own icon.
        }
        return new BitmapImage(new Uri("pack://application:,,,/Assets/apk-installer.png"));
    }

    private static BitmapImage Bitmap(byte[] data)
    {
        var image = new BitmapImage();
        image.BeginInit();
        image.CacheOption = BitmapCacheOption.OnLoad;
        image.StreamSource = new MemoryStream(data);
        image.EndInit();
        image.Freeze();
        return image;
    }

    private async void OnInstall(object sender, RoutedEventArgs e) => await _vm.InstallAsync();

    private async void OnOpen(object sender, RoutedEventArgs e)
    {
        OpenButton.IsEnabled = false;
        var why = await _vm.OpenAsync();
        OpenButton.IsEnabled = true;
        if (why is null)
        {
            Close();
        }
        else
        {
            ResultText.Text = why;
        }
    }

    private void OnCancel(object sender, RoutedEventArgs e)
    {
        if (_vm.IsBusy)
        {
            _vm.Cancel();
        }
        else
        {
            Close();
        }
    }

    private void OnClose(object sender, RoutedEventArgs e) => Close();

    protected override void OnClosing(CancelEventArgs e)
    {
        // Closing during an install stops it, as Cancel does; the window closes once it has.
        if (_vm.IsBusy)
        {
            _vm.Cancel();
        }
        base.OnClosing(e);
    }
}

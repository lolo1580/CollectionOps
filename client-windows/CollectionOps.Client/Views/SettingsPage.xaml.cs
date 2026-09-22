using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace CollectionOps.Client.Views;

public sealed partial class SettingsPage : Page
{
    public SettingsPage()
    {
        InitializeComponent();
        ServerAddress.Text = App.Sessions.ServerAddress ?? "http://127.0.0.1:8080";
        ThemeChoice.SelectedIndex = App.MainWindow?.CurrentTheme switch
        {
            ElementTheme.Light => 1,
            ElementTheme.Dark => 2,
            _ => 0,
        };
    }

    private async void OnCheckServer(object sender, RoutedEventArgs e)
    {
        CheckServerButton.IsEnabled = false;
        try
        {
            App.Sessions.Configure(ServerAddress.Text);
            var message = await App.Sessions.CheckHealthAsync();
            ShowStatus(message, InfoBarSeverity.Success);
        }
        catch (Exception error) when (error is HttpRequestException or TaskCanceledException or ArgumentException or InvalidOperationException)
        {
            ShowStatus(error is TaskCanceledException ? "Le serveur ne répond pas dans le délai prévu." : error.Message, InfoBarSeverity.Error);
        }
        finally
        {
            CheckServerButton.IsEnabled = true;
        }
    }

    private void OnThemeChanged(object sender, SelectionChangedEventArgs e)
    {
        if (ThemeChoice is null)
        {
            return;
        }

        App.MainWindow?.SetTheme(ThemeChoice.SelectedIndex switch
        {
            1 => ElementTheme.Light,
            2 => ElementTheme.Dark,
            _ => ElementTheme.Default,
        });
    }

    private void ShowStatus(string message, InfoBarSeverity severity)
    {
        StatusBar.Message = message;
        StatusBar.Severity = severity;
        StatusBar.IsOpen = true;
    }
}

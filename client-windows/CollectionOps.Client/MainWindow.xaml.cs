using CollectionOps.Client.Views;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace CollectionOps.Client;

public sealed partial class MainWindow : Window
{
    public MainWindow()
    {
        InitializeComponent();
        if (Navigation.SettingsItem is NavigationViewItem settingsItem)
        {
            settingsItem.Content = "Paramètres";
        }
    }

    public void SetTheme(ElementTheme theme) => Navigation.RequestedTheme = theme;
    public ElementTheme CurrentTheme => Navigation.RequestedTheme;

    private void OnSelectionChanged(NavigationView sender, NavigationViewSelectionChangedEventArgs args)
    {
        if (args.IsSettingsSelected)
        {
            ContentFrame.Navigate(typeof(SettingsPage));
            return;
        }

        if (args.SelectedItemContainer is not NavigationViewItem item)
        {
            return;
        }

        ContentFrame.Navigate((item.Tag as string) switch
        {
            "dashboard" => typeof(DashboardPage),
            "collection" => typeof(CollectionPage),
            "account" => typeof(AccountPage),
            _ => typeof(DashboardPage),
        }, item.Content);
    }
}


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

    public void OpenSection(string tag)
    {
        var item = Navigation.MenuItems.OfType<NavigationViewItem>()
            .FirstOrDefault(candidate => candidate.Tag as string == tag);
        if (item is not null) Navigation.SelectedItem = item;
    }

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
            "inventory" or "documents" or "acquisitions" or "organization" or "sharing" => typeof(CollectionPage),
            "finance" => typeof(FutureModulePage),
            "account" => typeof(AccountPage),
            _ => typeof(DashboardPage),
        }, item.Tag);
    }
}


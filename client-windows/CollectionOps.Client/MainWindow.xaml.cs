using CollectionOps.Client.Views;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace CollectionOps.Client;

public sealed partial class MainWindow : Window
{
    public MainWindow()
    {
        InitializeComponent();
    }

    private void OnSelectionChanged(NavigationView sender, NavigationViewSelectionChangedEventArgs args)
    {
        if (args.IsSettingsSelected)
        {
            ContentFrame.Navigate(typeof(PlaceholderPage), "Paramètres");
            return;
        }

        if (args.SelectedItemContainer is not NavigationViewItem item)
        {
            return;
        }

        ContentFrame.Navigate(item.Tag as string switch
        {
            "dashboard" => typeof(DashboardPage),
            "collection" => typeof(PlaceholderPage),
            _ => typeof(DashboardPage),
        }, item.Content);
    }
}


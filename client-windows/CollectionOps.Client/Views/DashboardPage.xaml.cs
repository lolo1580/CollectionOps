using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace CollectionOps.Client.Views;

public sealed partial class DashboardPage : Page
{
    public DashboardPage()
    {
        InitializeComponent();
        SessionStatus.Text = App.Sessions.IsSignedIn
            ? "Vous êtes connecté. Ouvrez votre collection pour retrouver vos objets."
            : "Connectez-vous pour afficher et gérer votre collection.";
    }

    private void OnOpenCollection(object sender, RoutedEventArgs e) => App.MainWindow?.OpenSection("inventory");

    private void OnOpenAccount(object sender, RoutedEventArgs e) => App.MainWindow?.OpenSection("account");
}


using CollectionOps.Client.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace CollectionOps.Client.Views;

public sealed partial class AccountPage : Page
{
    private SessionApi Api => App.Sessions;

    public AccountPage()
    {
        InitializeComponent();
        ServerAddress.Text = Api.ServerAddress ?? ServerAddress.Text;
        UpdateButtons();
        if (Api.IsSignedIn)
        {
            _ = RefreshSessionsAsync();
        }
    }

    private async void OnCheckServer(object sender, RoutedEventArgs e)
    {
        await RunAsync(async () =>
        {
            Api.Configure(ServerAddress.Text);
            ShowStatus(await Api.CheckHealthAsync(), InfoBarSeverity.Success);
        });
    }

    private async void OnSignIn(object sender, RoutedEventArgs e)
    {
        await RunAsync(async () =>
        {
            Api.Configure(ServerAddress.Text);
            await Api.SignInAsync(EmailAddress.Text.Trim(), Password.Password);
            Password.Password = string.Empty;
            ShowStatus("Connecté. Le jeton de cet appareil reste en mémoire jusqu'à la fermeture du client ou sa révocation.", InfoBarSeverity.Success);
            await RefreshSessionsAsync();
        });
        Password.Password = string.Empty;
    }

    private void OnSignOut(object sender, RoutedEventArgs e)
    {
        Api.SignOut();
        SessionsList.ItemsSource = null;
        UpdateButtons();
        ShowStatus("Déconnecté de ce client. La session serveur peut être révoquée depuis un autre appareil.", InfoBarSeverity.Informational);
    }

    private async void OnAcceptInvitation(object sender, RoutedEventArgs e)
    {
        await RunAsync(async () =>
        {
            Api.Configure(ServerAddress.Text);
            await Api.AcceptInvitationAsync(InvitationLink.Text,
                Api.IsSignedIn ? null : InvitedName.Text,
                Api.IsSignedIn ? null : InvitedPassword.Password);
            InvitationLink.Text = string.Empty;
            InvitedPassword.Password = string.Empty;
            ShowStatus(Api.IsSignedIn
                ? "Invitation acceptée. L'espace est maintenant accessible dans Collection."
                : "Compte créé et invitation acceptée. Connectez-vous avec votre adresse e-mail et votre mot de passe.",
                InfoBarSeverity.Success);
        });
        InvitedPassword.Password = string.Empty;
    }

    private async void OnRefresh(object sender, RoutedEventArgs e) => await RefreshSessionsAsync();

    private async void OnRevoke(object sender, RoutedEventArgs e)
    {
        if (sender is not Button { Tag: string sessionId })
        {
            return;
        }

        await RunAsync(async () =>
        {
            await Api.RevokeAsync(sessionId);
            if (Api.IsSignedIn)
            {
                await RefreshSessionsAsync();
            }
            else
            {
                SessionsList.ItemsSource = null;
                ShowStatus("Session de cet appareil révoquée.", InfoBarSeverity.Success);
            }
        });
    }

    private Task RefreshSessionsAsync() => RunAsync(async () =>
    {
        SessionsList.ItemsSource = (await Api.GetSessionsAsync())
            .Where(session => session.RevokedAt is null)
            .ToList();
    });

    private async Task RunAsync(Func<Task> action)
    {
        CheckServerButton.IsEnabled = false;
        SignInButton.IsEnabled = false;
        RefreshButton.IsEnabled = false;
        try
        {
            await action();
        }
        catch (Exception error) when (error is HttpRequestException or TaskCanceledException or ArgumentException or InvalidOperationException or System.Text.Json.JsonException or NotSupportedException)
        {
            if (!Api.IsSignedIn)
            {
                SessionsList.ItemsSource = null;
            }
            var message = error switch
            {
                TaskCanceledException => "Le serveur ne répond pas dans le délai prévu.",
                System.Text.Json.JsonException or NotSupportedException => "La réponse du serveur n'est pas un document JSON attendu.",
                _ => error.Message,
            };
            ShowStatus(message, InfoBarSeverity.Error);
        }
        finally
        {
            CheckServerButton.IsEnabled = true;
            UpdateButtons();
        }
    }

    private void UpdateButtons()
    {
        SignInButton.IsEnabled = !Api.IsSignedIn;
        SignOutButton.IsEnabled = Api.IsSignedIn;
        RefreshButton.IsEnabled = Api.IsSignedIn;
    }

    private void ShowStatus(string message, InfoBarSeverity severity)
    {
        StatusBar.Message = message;
        StatusBar.Severity = severity;
        StatusBar.IsOpen = true;
    }
}

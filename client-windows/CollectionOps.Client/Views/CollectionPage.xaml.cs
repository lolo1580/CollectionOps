using CollectionOps.Client.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace CollectionOps.Client.Views;

public sealed partial class CollectionPage : Page
{
    private SessionApi Api => App.Sessions;
    private CollectionSpace? ActiveSpace => Spaces.SelectedItem as CollectionSpace;

    public CollectionPage()
    {
        InitializeComponent();
        UpdateButtons();
        if (Api.IsSignedIn)
        {
            _ = RefreshSpacesAsync();
        }
    }

    private async void OnRefreshSpaces(object sender, RoutedEventArgs e) => await RefreshSpacesAsync();
    private async void OnRefreshItems(object sender, RoutedEventArgs e) => await RefreshItemsAsync();
    private async void OnSpaceSelected(object sender, SelectionChangedEventArgs e) => await RefreshItemsAsync();

    private async void OnCreateSpace(object sender, RoutedEventArgs e)
    {
        if (string.IsNullOrWhiteSpace(NewSpaceName.Text))
        {
            ShowStatus("Saisissez un nom d'espace.", InfoBarSeverity.Warning);
            return;
        }
        await RunAsync(async () =>
        {
            var space = await Api.CreateSpaceAsync(NewSpaceName.Text.Trim());
            NewSpaceName.Text = string.Empty;
            await LoadSpacesAsync(space.Id);
            ShowStatus("Espace créé.", InfoBarSeverity.Success);
        });
    }

    private async void OnCreateItem(object sender, RoutedEventArgs e)
    {
        if (ActiveSpace is null || string.IsNullOrWhiteSpace(NewItemName.Text))
        {
            ShowStatus("Choisissez un espace et saisissez un nom d'objet.", InfoBarSeverity.Warning);
            return;
        }
        var spaceId = ActiveSpace.Id;
        await RunAsync(async () =>
        {
            await Api.CreateItemAsync(spaceId, NewItemName.Text.Trim());
            NewItemName.Text = string.Empty;
            Items.ItemsSource = await Api.GetItemsAsync(spaceId);
            ShowStatus("Objet ajouté à l'inventaire.", InfoBarSeverity.Success);
        });
    }

    private Task RefreshSpacesAsync() => RunAsync(() => LoadSpacesAsync(ActiveSpace?.Id));

    private async Task LoadSpacesAsync(Guid? selectedId)
    {
        var spaces = await Api.GetSpacesAsync();
        Spaces.ItemsSource = spaces;
        Spaces.SelectedItem = spaces.FirstOrDefault(space => space.Id == selectedId) ?? spaces.FirstOrDefault();
        if (spaces.Count == 0)
        {
            Items.ItemsSource = null;
            ShowStatus("Aucun espace visible. Créez votre premier espace.", InfoBarSeverity.Informational);
        }
        UpdateButtons();
    }

    private Task RefreshItemsAsync() => RunAsync(async () =>
    {
        Items.ItemsSource = ActiveSpace is { } space
            ? await Api.GetItemsAsync(space.Id)
            : null;
    });

    private async Task RunAsync(Func<Task> action)
    {
        RefreshSpacesButton.IsEnabled = false;
        CreateSpaceButton.IsEnabled = false;
        CreateItemButton.IsEnabled = false;
        RefreshItemsButton.IsEnabled = false;
        try { await action(); }
        catch (Exception error) when (error is HttpRequestException or TaskCanceledException or ArgumentException or InvalidOperationException or System.Text.Json.JsonException or NotSupportedException)
        {
            ShowStatus(error is TaskCanceledException ? "Le serveur ne répond pas." : error.Message, InfoBarSeverity.Error);
        }
        finally { UpdateButtons(); }
    }

    private void UpdateButtons()
    {
        RefreshSpacesButton.IsEnabled = Api.IsSignedIn;
        CreateSpaceButton.IsEnabled = Api.IsSignedIn;
        CreateItemButton.IsEnabled = Api.IsSignedIn && ActiveSpace is not null;
        RefreshItemsButton.IsEnabled = Api.IsSignedIn && ActiveSpace is not null;
    }

    private void ShowStatus(string message, InfoBarSeverity severity)
    {
        StatusBar.Message = message;
        StatusBar.Severity = severity;
        StatusBar.IsOpen = true;
    }
}

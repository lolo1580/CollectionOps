using CollectionOps.Client.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace CollectionOps.Client.Views;

public sealed partial class CollectionPage : Page
{
    private SessionApi Api => App.Sessions;
    private CollectionSpace? ActiveSpace => Spaces.SelectedItem as CollectionSpace;
    private InventoryItem? ActiveItem => Items.SelectedItem as InventoryItem;
    private IReadOnlyList<CollectionSpace> _spaces = [];
    private bool _loadingSpaces;
    private string _search = string.Empty;
    private string? _nextCursor;

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
    private async void OnSearch(object sender, RoutedEventArgs e)
    {
        _search = SearchText.Text.Trim();
        await RefreshItemsAsync();
    }

    private async void OnLoadMore(object sender, RoutedEventArgs e)
    {
        if (ActiveSpace is not { } space || _nextCursor is not { } cursor) return;
        var search = _search;
        await RunAsync(async () =>
        {
            var page = await Api.GetItemsPageAsync(space.Id, search, cursor);
            if (ActiveSpace?.Id != space.Id || _search != search) return;
            var items = (Items.ItemsSource as IEnumerable<InventoryItem>)?.ToList() ?? [];
            items.AddRange(page.Items);
            Items.ItemsSource = items;
            _nextCursor = page.NextCursor;
        });
    }
    private async void OnSpaceSelected(object sender, SelectionChangedEventArgs e)
    {
        if (_loadingSpaces) return;
        await RunAsync(async () =>
        {
            UpdateDestinationOptions();
            TransferHistory.ItemsSource = null;
            await RefreshItemsCoreAsync();
        });
    }

    private async void OnItemSelected(object sender, SelectionChangedEventArgs e)
    {
        SelectedItemName.Text = ActiveItem is { } item
            ? $"{item.Name} — n° {item.InventoryNumber} (révision {item.Revision})"
            : "Sélectionnez un objet dans l'inventaire.";
        TransferHistory.ItemsSource = null;
        UpdateButtons();
        if (ActiveItem is not null) await RefreshHistoryAsync();
    }

    private async void OnRefreshHistory(object sender, RoutedEventArgs e) => await RefreshHistoryAsync();

    private async void OnTransferItem(object sender, RoutedEventArgs e)
    {
        if (ActiveItem is not { } item || DestinationSpaces.SelectedItem is not CollectionSpace destination)
        {
            ShowStatus("Sélectionnez un objet et un espace de destination.", InfoBarSeverity.Warning);
            return;
        }
        await RunAsync(async () =>
        {
            var outcome = await Api.TransferItemAsync(item.Id, destination.Id, item.Revision);
            _search = string.Empty;
            SearchText.Text = string.Empty;
            await LoadSpacesAsync(destination.Id);
            Items.SelectedItem = (Items.ItemsSource as IReadOnlyList<InventoryItem>)?
                .FirstOrDefault(candidate => candidate.Id == outcome.Item.Id);
            ShowStatus($"Objet transféré vers {destination.Name}, n° {outcome.Item.InventoryNumber}.", InfoBarSeverity.Success);
        });
    }

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
            await RefreshItemsCoreAsync();
            ShowStatus("Objet ajouté à l'inventaire.", InfoBarSeverity.Success);
        });
    }

    private Task RefreshSpacesAsync() => RunAsync(() => LoadSpacesAsync(ActiveSpace?.Id));

    private async Task LoadSpacesAsync(Guid? selectedId)
    {
        _spaces = await Api.GetSpacesAsync();
        _loadingSpaces = true;
        try
        {
            Spaces.ItemsSource = _spaces;
            Spaces.SelectedItem = _spaces.FirstOrDefault(space => space.Id == selectedId) ?? _spaces.FirstOrDefault();
        }
        finally { _loadingSpaces = false; }
        UpdateDestinationOptions();
        TransferHistory.ItemsSource = null;
        if (_spaces.Count == 0)
        {
            Items.ItemsSource = null;
            ShowStatus("Aucun espace visible. Créez votre premier espace.", InfoBarSeverity.Informational);
        }
        else await RefreshItemsCoreAsync();
        UpdateButtons();
    }

    private Task RefreshItemsAsync() => RunAsync(RefreshItemsCoreAsync);

    private async Task RefreshItemsCoreAsync()
    {
        _nextCursor = null;
        if (ActiveSpace is { } space)
        {
            var page = await Api.GetItemsPageAsync(space.Id, _search);
            Items.ItemsSource = page.Items;
            _nextCursor = page.NextCursor;
        }
        else Items.ItemsSource = null;
        Items.SelectedItem = null;
        TransferHistory.ItemsSource = null;
    }

    private Task RefreshHistoryAsync() => RunAsync(async () =>
    {
        if (ActiveItem is not { } item) return;
        var transfers = await Api.GetItemTransfersAsync(item.Id);
        if (ActiveItem?.Id != item.Id) return;
        TransferHistory.ItemsSource = transfers.Select(transfer => new TransferDisplay(
            $"{SpaceName(transfer.SourceSpaceId)} n° {transfer.SourceInventoryNumber} → " +
            $"{SpaceName(transfer.DestinationSpaceId)} n° {transfer.DestinationInventoryNumber}",
            transfer.TransferredAt.ToLocalTime().ToString("g"))).ToList();
    });

    private string SpaceName(Guid id) => _spaces.FirstOrDefault(space => space.Id == id)?.Name ?? "Espace inaccessible";

    private void UpdateDestinationOptions()
    {
        DestinationSpaces.ItemsSource = _spaces.Where(space => space.Id != ActiveSpace?.Id).ToList();
        DestinationSpaces.SelectedIndex = -1;
    }

    private async Task RunAsync(Func<Task> action)
    {
        RefreshSpacesButton.IsEnabled = false;
        CreateSpaceButton.IsEnabled = false;
        CreateItemButton.IsEnabled = false;
        RefreshItemsButton.IsEnabled = false;
        SearchButton.IsEnabled = false;
        LoadMoreButton.IsEnabled = false;
        TransferButton.IsEnabled = false;
        RefreshHistoryButton.IsEnabled = false;
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
        SearchButton.IsEnabled = Api.IsSignedIn && ActiveSpace is not null;
        LoadMoreButton.IsEnabled = Api.IsSignedIn && ActiveSpace is not null && _nextCursor is not null;
        TransferButton.IsEnabled = Api.IsSignedIn && ActiveItem is not null && DestinationSpaces.ItemsSource is not null;
        RefreshHistoryButton.IsEnabled = Api.IsSignedIn && ActiveItem is not null;
    }

    private void ShowStatus(string message, InfoBarSeverity severity)
    {
        StatusBar.Message = message;
        StatusBar.Severity = severity;
        StatusBar.IsOpen = true;
    }
}

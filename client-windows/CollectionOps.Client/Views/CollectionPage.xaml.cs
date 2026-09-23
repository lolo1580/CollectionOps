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
    private bool _canReadActiveSpace;

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
            await RefreshMembersCoreAsync();
            await RefreshItemsCoreAsync();
            await RefreshInvitationsCoreAsync();
        });
    }

    private async void OnItemSelected(object sender, SelectionChangedEventArgs e)
    {
        SelectedItemName.Text = ActiveItem is { } item
            ? $"{item.Name} — n° {item.InventoryNumber} (révision {item.Revision})"
            : "Sélectionnez un objet dans l'inventaire.";
        SelectedItemDetails.Text = ActiveItem is { } selected
            ? $"Identifiant : {selected.Id} · Créé le {selected.CreatedAt.ToLocalTime():g}"
            : string.Empty;
        EditedItemName.Text = ActiveItem?.Name ?? string.Empty;
        TransferHistory.ItemsSource = null;
        UpdateButtons();
        if (ActiveItem is not null) await RefreshHistoryAsync();
    }

    private async void OnSaveItemName(object sender, RoutedEventArgs e)
    {
        if (ActiveItem is not { } item) return;
        var name = EditedItemName.Text.Trim();
        if (name.Length == 0)
        {
            ShowStatus("Saisissez un nom d'objet.", InfoBarSeverity.Warning);
            return;
        }
        await RunAsync(async () =>
        {
            var updated = await Api.RenameItemAsync(item.Id, name, item.Revision);
            await RefreshItemsCoreAsync();
            Items.SelectedItem = (Items.ItemsSource as IReadOnlyList<InventoryItem>)?
                .FirstOrDefault(candidate => candidate.Id == updated.Id);
            ShowStatus("Nom de l'objet enregistré.", InfoBarSeverity.Success);
        });
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

    private async void OnInvite(object sender, RoutedEventArgs e)
    {
        if (ActiveSpace is not { } space || string.IsNullOrWhiteSpace(InviteEmail.Text))
        {
            ShowStatus("Choisissez un espace et une adresse e-mail.", InfoBarSeverity.Warning);
            return;
        }
        await RunAsync(async () =>
        {
            var grants = new List<string>();
            if (GrantCollectionsRead.IsChecked == true) grants.Add("collections_read");
            if (GrantCollectionsWrite.IsChecked == true) grants.Add("collections_write");
            if (GrantAcquisitionsRead.IsChecked == true) grants.Add("acquisitions_read");
            if (GrantAcquisitionsWrite.IsChecked == true) grants.Add("acquisitions_write");
            if (GrantFinanceRead.IsChecked == true) grants.Add("finance_read");
            if (GrantFinanceWrite.IsChecked == true) grants.Add("finance_write");
            if (GrantDocumentsRead.IsChecked == true) grants.Add("documents_read");
            if (GrantDocumentsWrite.IsChecked == true) grants.Add("documents_write");
            if (grants.Count == 0) throw new ArgumentException("Choisissez au moins un droit pour l'invitation.");
            await Api.InviteAsync(space.Id, InviteEmail.Text.Trim(), grants);
            InviteEmail.Text = string.Empty;
            await RefreshInvitationsCoreAsync();
            ShowStatus("Invitation transmise au serveur SMTP.", InfoBarSeverity.Success);
        });
    }

    private async void OnRefreshInvitations(object sender, RoutedEventArgs e) =>
        await RunAsync(RefreshInvitationsCoreAsync);

    private async void OnRevokeInvitation(object sender, RoutedEventArgs e)
    {
        if (ActiveSpace is not { } space || sender is not Button { Tag: Guid invitationId }) return;
        await RunAsync(async () =>
        {
            await Api.RevokeInvitationAsync(space.Id, invitationId);
            await RefreshInvitationsCoreAsync();
            ShowStatus("Invitation révoquée.", InfoBarSeverity.Success);
        });
    }

    private async Task RefreshInvitationsCoreAsync()
    {
        var space = ActiveSpace;
        SharingPanel.Visibility = space is not null && space.OwnerAccountId == Api.CurrentAccountId
            ? Visibility.Visible : Visibility.Collapsed;
        Invitations.ItemsSource = null;
        if (SharingPanel.Visibility == Visibility.Visible && space is not null)
            Invitations.ItemsSource = await Api.GetInvitationsAsync(space.Id);
    }

    private async void OnRefreshMembers(object sender, RoutedEventArgs e) =>
        await RunAsync(() => RefreshMembersCoreAsync((Members.SelectedItem as SpaceMember)?.AccountId));

    private void OnMemberSelected(object sender, SelectionChangedEventArgs e)
    {
        var grants = (Members.SelectedItem as SpaceMember)?.Permissions ?? [];
        MemberCollectionsRead.IsChecked = grants.Contains("collections_read");
        MemberCollectionsWrite.IsChecked = grants.Contains("collections_write");
        MemberAcquisitionsRead.IsChecked = grants.Contains("acquisitions_read");
        MemberAcquisitionsWrite.IsChecked = grants.Contains("acquisitions_write");
        MemberFinanceRead.IsChecked = grants.Contains("finance_read");
        MemberFinanceWrite.IsChecked = grants.Contains("finance_write");
        MemberDocumentsRead.IsChecked = grants.Contains("documents_read");
        MemberDocumentsWrite.IsChecked = grants.Contains("documents_write");
        UpdateButtons();
    }

    private async void OnSaveMemberRights(object sender, RoutedEventArgs e)
    {
        if (ActiveSpace is not { } space || Members.SelectedItem is not SpaceMember member) return;
        var grants = new List<string>();
        if (MemberCollectionsRead.IsChecked == true) grants.Add("collections_read");
        if (MemberCollectionsWrite.IsChecked == true) grants.Add("collections_write");
        if (MemberAcquisitionsRead.IsChecked == true) grants.Add("acquisitions_read");
        if (MemberAcquisitionsWrite.IsChecked == true) grants.Add("acquisitions_write");
        if (MemberFinanceRead.IsChecked == true) grants.Add("finance_read");
        if (MemberFinanceWrite.IsChecked == true) grants.Add("finance_write");
        if (MemberDocumentsRead.IsChecked == true) grants.Add("documents_read");
        if (MemberDocumentsWrite.IsChecked == true) grants.Add("documents_write");
        await RunAsync(async () =>
        {
            await Api.UpdateMemberPermissionsAsync(space.Id, member.AccountId, grants);
            await RefreshMembersCoreAsync(member.AccountId);
            ShowStatus("Droits du membre mis à jour.", InfoBarSeverity.Success);
        });
    }

    private async void OnRemoveMember(object sender, RoutedEventArgs e)
    {
        if (ActiveSpace is not { } space || Members.SelectedItem is not SpaceMember member) return;
        var dialog = new ContentDialog
        {
            XamlRoot = XamlRoot,
            Title = "Retirer ce membre ?",
            Content = $"{member.Label} perdra immédiatement l'accès à cet espace.",
            PrimaryButtonText = "Retirer",
            CloseButtonText = "Annuler",
            DefaultButton = ContentDialogButton.Close,
        };
        if (await dialog.ShowAsync() != ContentDialogResult.Primary) return;
        await RunAsync(async () =>
        {
            await Api.RemoveMemberAsync(space.Id, member.AccountId);
            await RefreshMembersCoreAsync();
            ShowStatus("Membre retiré de l'espace.", InfoBarSeverity.Success);
        });
    }

    private async Task RefreshMembersCoreAsync(Guid? selectedId = null)
    {
        var space = ActiveSpace;
        _canReadActiveSpace = !Api.CurrentIsSystemAdmin && space is not null;
        MembersPanel.Visibility = space is not null &&
            (space.OwnerAccountId == Api.CurrentAccountId || Api.CurrentIsSystemAdmin)
            ? Visibility.Visible : Visibility.Collapsed;
        Members.ItemsSource = null;
        if (MembersPanel.Visibility != Visibility.Visible || space is null) return;
        var members = await Api.GetMembersAsync(space.Id);
        _canReadActiveSpace = members.Any(member => member.AccountId == Api.CurrentAccountId &&
            member.Permissions.Contains("collections_read"));
        Members.ItemsSource = members;
        Members.SelectedItem = members.FirstOrDefault(member => member.AccountId == selectedId)
            ?? members.FirstOrDefault();
    }

    private Task RefreshSpacesAsync() => RunAsync(() => LoadSpacesAsync(ActiveSpace?.Id));

    private async Task LoadSpacesAsync(Guid? selectedId)
    {
        _spaces = Api.CurrentIsSystemAdmin ? await Api.GetAdminSpacesAsync() : await Api.GetSpacesAsync();
        _loadingSpaces = true;
        try
        {
            Spaces.ItemsSource = _spaces;
            Spaces.SelectedItem = _spaces.FirstOrDefault(space => space.Id == selectedId) ?? _spaces.FirstOrDefault();
        }
        finally { _loadingSpaces = false; }
        UpdateDestinationOptions();
        TransferHistory.ItemsSource = null;
        await RefreshMembersCoreAsync();
        if (_spaces.Count == 0)
        {
            Items.ItemsSource = null;
            ShowStatus("Aucun espace visible. Créez votre premier espace.", InfoBarSeverity.Informational);
        }
        else await RefreshItemsCoreAsync();
        await RefreshInvitationsCoreAsync();
        UpdateButtons();
    }

    private Task RefreshItemsAsync() => RunAsync(RefreshItemsCoreAsync);

    private async Task RefreshItemsCoreAsync()
    {
        _nextCursor = null;
        if (ActiveSpace is { } space && _canReadActiveSpace)
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
        SaveItemNameButton.IsEnabled = false;
        InviteButton.IsEnabled = false;
        RefreshInvitationsButton.IsEnabled = false;
        SaveMemberRightsButton.IsEnabled = false;
        RemoveMemberButton.IsEnabled = false;
        RefreshMembersButton.IsEnabled = false;
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
        CreateItemButton.IsEnabled = Api.IsSignedIn && ActiveSpace is not null && _canReadActiveSpace;
        RefreshItemsButton.IsEnabled = Api.IsSignedIn && ActiveSpace is not null && _canReadActiveSpace;
        SearchButton.IsEnabled = Api.IsSignedIn && ActiveSpace is not null && _canReadActiveSpace;
        LoadMoreButton.IsEnabled = Api.IsSignedIn && ActiveSpace is not null && _canReadActiveSpace && _nextCursor is not null;
        TransferButton.IsEnabled = Api.IsSignedIn && ActiveItem is not null && DestinationSpaces.ItemsSource is not null;
        RefreshHistoryButton.IsEnabled = Api.IsSignedIn && ActiveItem is not null;
        SaveItemNameButton.IsEnabled = Api.IsSignedIn && ActiveItem is not null;
        InviteButton.IsEnabled = SharingPanel.Visibility == Visibility.Visible;
        RefreshInvitationsButton.IsEnabled = SharingPanel.Visibility == Visibility.Visible;
        RefreshMembersButton.IsEnabled = MembersPanel.Visibility == Visibility.Visible;
        var canChangeMember = MembersPanel.Visibility == Visibility.Visible &&
            Members.SelectedItem is SpaceMember member && member.AccountId != Api.CurrentAccountId;
        SaveMemberRightsButton.IsEnabled = canChangeMember;
        RemoveMemberButton.IsEnabled = canChangeMember &&
            (Members.SelectedItem as SpaceMember)?.AccountId != ActiveSpace?.OwnerAccountId;
    }

    private void ShowStatus(string message, InfoBarSeverity severity)
    {
        StatusBar.Message = message;
        StatusBar.Severity = severity;
        StatusBar.IsOpen = true;
    }
}

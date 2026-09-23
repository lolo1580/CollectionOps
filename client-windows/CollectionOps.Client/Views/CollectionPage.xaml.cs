using CollectionOps.Client.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace CollectionOps.Client.Views;

public sealed partial class CollectionPage : Page
{
    private SessionApi Api => App.Sessions;
    private CollectionSpace? ActiveSpace => Spaces.SelectedItem as CollectionSpace;
    private InventoryItem? ActiveItem => Items.SelectedItem as InventoryItem;
    private bool CanReadStateAudit => ActiveSpace is { } space &&
        (space.OwnerAccountId == Api.CurrentAccountId || Api.CurrentIsSystemAdmin);
    private IReadOnlyList<CollectionSpace> _spaces = [];
    private bool _loadingSpaces;
    private string _search = string.Empty;
    private string _stateFilter = "active";
    private string? _nextCursor;
    private bool _canReadActiveSpace;
    private bool _loadingCategories;
    private Guid? _categoryFilter;
    private Guid? _locationFilter;
    private bool _loadingLocations;
    private IReadOnlyList<CollectionCategory> _categories = [];
    private IReadOnlyList<CollectionLocation> _locations = [];
    private int _assignedCategoryCount;
    private HashSet<Guid> _assignedCategoryIds = [];

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

    private async void OnStateFilterChanged(object sender, SelectionChangedEventArgs e)
    {
        if (!Api.IsSignedIn) return;
        _stateFilter = (StateFilter.SelectedItem as ComboBoxItem)?.Tag as string ?? "active";
        await RefreshItemsAsync();
    }

    private async void OnCategoryFilterChanged(object sender, SelectionChangedEventArgs e)
    {
        if (_loadingCategories || !Api.IsSignedIn) return;
        _categoryFilter = (CategoryFilter.SelectedItem as CategoryOption)?.Id;
        await RefreshItemsAsync();
    }

    private async void OnLocationFilterChanged(object sender, SelectionChangedEventArgs e)
    {
        if (_loadingLocations || !Api.IsSignedIn) return;
        _locationFilter = (LocationFilter.SelectedItem as LocationOption)?.Id;
        await RefreshItemsAsync();
    }

    private async void OnLoadMore(object sender, RoutedEventArgs e)
    {
        if (ActiveSpace is not { } space || _nextCursor is not { } cursor) return;
        var search = _search;
        var state = _stateFilter;
        var categoryId = _categoryFilter;
        var locationId = _locationFilter;
        await RunAsync(async () =>
        {
            var page = await Api.GetItemsPageAsync(space.Id, search, cursor, state: state,
                categoryId: categoryId, locationId: locationId);
            if (ActiveSpace?.Id != space.Id || _search != search || _stateFilter != state ||
                _categoryFilter != categoryId || _locationFilter != locationId) return;
            var items = (Items.ItemsSource as IEnumerable<InventoryItem>)?.ToList() ?? [];
            items.AddRange(page.Items);
            Items.ItemsSource = items;
            _nextCursor = page.NextCursor;
        });
    }
    private async void OnSpaceSelected(object sender, SelectionChangedEventArgs e)
    {
        if (_loadingSpaces) return;
        _categoryFilter = null;
        _locationFilter = null;
        await RunAsync(async () =>
        {
            UpdateDestinationOptions();
            TransferHistory.ItemsSource = null;
            await RefreshMembersCoreAsync();
            await RefreshCategoriesCoreAsync();
            await RefreshLocationsCoreAsync();
            await RefreshItemsCoreAsync();
            await RefreshInvitationsCoreAsync();
        });
    }

    private async void OnItemSelected(object sender, SelectionChangedEventArgs e)
    {
        SelectedItemName.Text = ActiveItem is { } item
            ? $"{item.Name} — n° {item.InventoryNumber} (révision {item.Revision}, {item.StateLabel})"
            : "Sélectionnez un objet dans l'inventaire.";
        SelectedItemDetails.Text = ActiveItem is { } selected
            ? $"Identifiant : {selected.Id} · Créé le {selected.CreatedAt.ToLocalTime():g}"
            : string.Empty;
        EditedItemName.Text = ActiveItem?.Name ?? string.Empty;
        TransferHistory.ItemsSource = null;
        StateAuditHistory.ItemsSource = null;
        AssignedCategories.Text = "Aucune catégorie.";
        EffectiveFields.ItemsSource = null;
        CurrentLocationText.Text = "Aucun emplacement.";
        LocationHistory.ItemsSource = null;
        _assignedCategoryCount = 0;
        _assignedCategoryIds.Clear();
        StateAuditPanel.Visibility = ActiveItem is not null && CanReadStateAudit
            ? Visibility.Visible : Visibility.Collapsed;
        UpdateButtons();
        if (ActiveItem is not null)
        {
            await RefreshHistoryAsync();
            await RefreshItemTaxonomyAsync();
            await RefreshItemLocationAsync();
            if (CanReadStateAudit) await RefreshStateAuditAsync();
        }
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
    private async void OnRefreshStateAudit(object sender, RoutedEventArgs e) => await RefreshStateAuditAsync();

    private async void OnArchiveItem(object sender, RoutedEventArgs e)
    {
        if (ActiveItem is not { } item) return;
        await RunAsync(async () =>
        {
            var updated = await Api.ArchiveItemAsync(item.Id, item.Revision);
            await RefreshItemsCoreAsync();
            ShowStatus($"Objet archivé (révision {updated.Revision}).", InfoBarSeverity.Success);
        });
    }

    private async void OnTrashItem(object sender, RoutedEventArgs e)
    {
        if (ActiveItem is not { } item) return;
        var dialog = new ContentDialog
        {
            XamlRoot = XamlRoot,
            Title = "Mettre cet objet à la corbeille ?",
            Content = $"{item.Name} quittera la vue courante. Il pourra être restauré depuis la corbeille.",
            PrimaryButtonText = "Mettre à la corbeille",
            CloseButtonText = "Annuler",
            DefaultButton = ContentDialogButton.Close,
        };
        if (await dialog.ShowAsync() != ContentDialogResult.Primary) return;
        await RunAsync(async () =>
        {
            var updated = await Api.TrashItemAsync(item.Id, item.Revision);
            await RefreshItemsCoreAsync();
            ShowStatus($"Objet mis à la corbeille (révision {updated.Revision}).", InfoBarSeverity.Success);
        });
    }

    private async void OnRestoreItem(object sender, RoutedEventArgs e)
    {
        if (ActiveItem is not { } item) return;
        await RunAsync(async () =>
        {
            var updated = await Api.RestoreItemAsync(item.Id, item.Revision);
            await RefreshItemsCoreAsync();
            ShowStatus($"Objet restauré (révision {updated.Revision}).", InfoBarSeverity.Success);
        });
    }

    private async void OnTransferItem(object sender, RoutedEventArgs e)
    {
        if (ActiveItem is not { } item || DestinationSpaces.SelectedItem is not CollectionSpace destination)
        {
            ShowStatus("Sélectionnez un objet et un espace de destination.", InfoBarSeverity.Warning);
            return;
        }
        var selectedCategories = DestinationCategoryChoices.SelectedItems.Cast<CategoryOption>()
            .Where(option => option.Id.HasValue).Select(option => option.Id!.Value).ToList();
        if (_assignedCategoryCount > 0 && selectedCategories.Count == 0)
        {
            ShowStatus("Choisissez au moins une catégorie dans l'espace de destination.", InfoBarSeverity.Warning);
            return;
        }
        await RunAsync(async () =>
        {
            var outcome = await Api.TransferItemAsync(item.Id, destination.Id, item.Revision, selectedCategories);
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
        var previousSpaceId = ActiveSpace?.Id;
        _spaces = Api.CurrentIsSystemAdmin ? await Api.GetAdminSpacesAsync() : await Api.GetSpacesAsync();
        _loadingSpaces = true;
        try
        {
            Spaces.ItemsSource = _spaces;
            Spaces.SelectedItem = _spaces.FirstOrDefault(space => space.Id == selectedId) ?? _spaces.FirstOrDefault();
        }
        finally { _loadingSpaces = false; }
        if (previousSpaceId != ActiveSpace?.Id)
        {
            _categoryFilter = null;
            _locationFilter = null;
        }
        UpdateDestinationOptions();
        TransferHistory.ItemsSource = null;
        await RefreshMembersCoreAsync();
        await RefreshCategoriesCoreAsync();
        await RefreshLocationsCoreAsync();
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
            var page = await Api.GetItemsPageAsync(space.Id, _search, state: _stateFilter,
                categoryId: _categoryFilter, locationId: _locationFilter);
            Items.ItemsSource = page.Items;
            _nextCursor = page.NextCursor;
        }
        else Items.ItemsSource = null;
        Items.SelectedItem = null;
        TransferHistory.ItemsSource = null;
        StateAuditHistory.ItemsSource = null;
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

    private Task RefreshStateAuditAsync() => RunAsync(async () =>
    {
        if (ActiveItem is not { } item || !CanReadStateAudit) return;
        var events = await Api.GetItemStateEventsAsync(item.Id);
        if (ActiveItem?.Id != item.Id) return;
        StateAuditHistory.ItemsSource = events;
    });

    private string SpaceName(Guid id) => _spaces.FirstOrDefault(space => space.Id == id)?.Name ?? "Espace inaccessible";

    private void UpdateDestinationOptions()
    {
        DestinationSpaces.ItemsSource = _spaces.Where(space => space.Id != ActiveSpace?.Id).ToList();
        DestinationSpaces.SelectedIndex = -1;
        DestinationCategoryChoices.ItemsSource = null;
    }

    private static List<CategoryOption> CategoryOptions(IReadOnlyList<CollectionCategory> categories)
    {
        var byId = categories.ToDictionary(category => category.Id);
        string Label(CollectionCategory category)
        {
            var names = new List<string> { category.Name };
            var seen = new HashSet<Guid> { category.Id };
            var parent = category.ParentId;
            while (parent is { } id && seen.Add(id) && byId.TryGetValue(id, out var ancestor))
            {
                names.Insert(0, ancestor.Name);
                parent = ancestor.ParentId;
            }
            return string.Join(" / ", names);
        }
        return categories.Select(category => new CategoryOption(category.Id, Label(category)))
            .OrderBy(option => option.Label, StringComparer.CurrentCultureIgnoreCase).ToList();
    }

    private static List<LocationOption> LocationOptions(IReadOnlyList<CollectionLocation> locations)
    {
        var byId = locations.ToDictionary(location => location.Id);
        string Label(CollectionLocation location)
        {
            var names = new List<string> { location.Name };
            var seen = new HashSet<Guid> { location.Id };
            var parent = location.ParentId;
            while (parent is { } id && seen.Add(id) && byId.TryGetValue(id, out var ancestor))
            {
                names.Insert(0, ancestor.Name);
                parent = ancestor.ParentId;
            }
            return string.Join(" / ", names);
        }
        return locations.Select(location => new LocationOption(location.Id, Label(location)))
            .OrderBy(option => option.Label, StringComparer.CurrentCultureIgnoreCase).ToList();
    }

    private async Task RefreshCategoriesCoreAsync()
    {
        _categories = ActiveSpace is { } space && _canReadActiveSpace
            ? await Api.GetCategoriesAsync(space.Id) : [];
        var options = CategoryOptions(_categories);
        _loadingCategories = true;
        try
        {
            CategoryFilter.ItemsSource = new[] { new CategoryOption(null, "Toutes les catégories") }
                .Concat(options).ToList();
            CategoryFilter.SelectedItem = (CategoryFilter.ItemsSource as IEnumerable<CategoryOption>)?
                .FirstOrDefault(option => option.Id == _categoryFilter);
        }
        finally { _loadingCategories = false; }
        ParentCategories.ItemsSource = new[] { new CategoryOption(null, "Aucune — catégorie racine") }
            .Concat(options).ToList();
        ParentCategories.SelectedIndex = 0;
        FieldCategory.ItemsSource = options;
        ItemCategoryChoices.ItemsSource = options;
        CategoryFields.ItemsSource = null;
    }

    private async Task RefreshLocationsCoreAsync()
    {
        _locations = ActiveSpace is { } space && _canReadActiveSpace
            ? await Api.GetLocationsAsync(space.Id) : [];
        var options = LocationOptions(_locations);
        _loadingLocations = true;
        try
        {
            LocationFilter.ItemsSource = new[] { new LocationOption(null, "Tous les emplacements") }
                .Concat(options).ToList();
            LocationFilter.SelectedItem = (LocationFilter.ItemsSource as IEnumerable<LocationOption>)?
                .FirstOrDefault(option => option.Id == _locationFilter);
        }
        finally { _loadingLocations = false; }
        ParentLocations.ItemsSource = new[] { new LocationOption(null, "Aucun — emplacement racine") }
            .Concat(options).ToList();
        ParentLocations.SelectedIndex = 0;
        ItemLocationChoices.ItemsSource = new[] { new LocationOption(null, "Sans emplacement") }
            .Concat(options).ToList();
        ItemLocationChoices.SelectedIndex = 0;
    }

    private async void OnCreateLocation(object sender, RoutedEventArgs e)
    {
        if (ActiveSpace is not { } space || string.IsNullOrWhiteSpace(NewLocationName.Text)) return;
        var parentId = (ParentLocations.SelectedItem as LocationOption)?.Id;
        await RunAsync(async () =>
        {
            await Api.CreateLocationAsync(space.Id, NewLocationName.Text.Trim(), parentId);
            NewLocationName.Text = string.Empty;
            await RefreshLocationsCoreAsync();
            if (ActiveItem is not null) await RefreshItemLocationCoreAsync();
            ShowStatus("Emplacement créé.", InfoBarSeverity.Success);
        });
    }

    private async void OnCreateCategory(object sender, RoutedEventArgs e)
    {
        if (ActiveSpace is not { } space || string.IsNullOrWhiteSpace(NewCategoryName.Text)) return;
        var parentId = (ParentCategories.SelectedItem as CategoryOption)?.Id;
        await RunAsync(async () =>
        {
            await Api.CreateCategoryAsync(space.Id, NewCategoryName.Text.Trim(), parentId);
            NewCategoryName.Text = string.Empty;
            await RefreshCategoriesCoreAsync();
            ShowStatus("Catégorie créée.", InfoBarSeverity.Success);
        });
    }

    private async void OnFieldCategorySelected(object sender, SelectionChangedEventArgs e)
    {
        if (ActiveSpace is not { } space || FieldCategory.SelectedItem is not CategoryOption { Id: { } id }) return;
        await RunAsync(async () => CategoryFields.ItemsSource = await Api.GetCategoryFieldsAsync(space.Id, id));
    }

    private async void OnCreateField(object sender, RoutedEventArgs e)
    {
        if (ActiveSpace is not { } space || FieldCategory.SelectedItem is not CategoryOption { Id: { } id } ||
            string.IsNullOrWhiteSpace(NewFieldName.Text)) return;
        var type = (NewFieldType.SelectedItem as ComboBoxItem)?.Tag as string ?? "text";
        await RunAsync(async () =>
        {
            await Api.CreateCategoryFieldAsync(space.Id, id, NewFieldName.Text.Trim(), type);
            NewFieldName.Text = string.Empty;
            CategoryFields.ItemsSource = await Api.GetCategoryFieldsAsync(space.Id, id);
            ShowStatus("Champ créé et hérité par les sous-catégories.", InfoBarSeverity.Success);
        });
    }

    private Task RefreshItemTaxonomyAsync() => RunAsync(RefreshItemTaxonomyCoreAsync);

    private async Task RefreshItemTaxonomyCoreAsync()
    {
        if (ActiveItem is not { } item) return;
        var categories = await Api.GetItemCategoriesAsync(item.Id);
        var fields = await Api.GetItemFieldsAsync(item.Id);
        if (ActiveItem?.Id != item.Id) return;
        _assignedCategoryCount = categories.Categories.Count;
        _assignedCategoryIds = categories.Categories.Select(category => category.CategoryId).ToHashSet();
        AssignedCategories.Text = _assignedCategoryCount == 0 ? "Aucune catégorie." :
            "Catégories : " + string.Join(", ", categories.Categories.Select(category => category.CategoryName));
        EffectiveFields.ItemsSource = fields.Fields;
        ItemCategoryChoices.SelectedItems.Clear();
        foreach (var option in (ItemCategoryChoices.ItemsSource as IEnumerable<CategoryOption>) ?? [])
            if (option.Id is { } id && _assignedCategoryIds.Contains(id))
                ItemCategoryChoices.SelectedItems.Add(option);
    }

    private Task RefreshItemLocationAsync() => RunAsync(RefreshItemLocationCoreAsync);

    private async Task RefreshItemLocationCoreAsync()
    {
        if (ActiveItem is not { } item) return;
        var current = await Api.GetItemLocationAsync(item.Id);
        var events = await Api.GetItemLocationEventsAsync(item.Id);
        if (ActiveItem?.Id != item.Id) return;
        var choice = (ItemLocationChoices.ItemsSource as IEnumerable<LocationOption>)?
            .FirstOrDefault(option => option.Id == current.Location?.Id);
        ItemLocationChoices.SelectedItem = choice;
        CurrentLocationText.Text = current.Location is null ? "Aucun emplacement." :
            "Emplacement : " + (choice?.Label ?? current.Location.Name);
        LocationHistory.ItemsSource = events;
    }

    private async void OnMoveItemLocation(object sender, RoutedEventArgs e)
    {
        if (ActiveItem is not { } item || ItemLocationChoices.SelectedItem is not LocationOption choice) return;
        await RunAsync(async () =>
        {
            await Api.MoveItemLocationAsync(item.Id, choice.Id, item.Revision);
            await RefreshItemsCoreAsync();
            Items.SelectedItem = (Items.ItemsSource as IReadOnlyList<InventoryItem>)?
                .FirstOrDefault(candidate => candidate.Id == item.Id);
            ShowStatus("Emplacement enregistré.", InfoBarSeverity.Success);
        });
    }

    private async void OnSaveItemCategories(object sender, RoutedEventArgs e)
    {
        if (ActiveItem is not { } item) return;
        var ids = ItemCategoryChoices.SelectedItems.Cast<CategoryOption>()
            .Where(option => option.Id.HasValue).Select(option => option.Id!.Value).ToList();
        var removing = _assignedCategoryIds.Except(ids).Any();
        if (removing)
        {
            var dialog = new ContentDialog
            {
                XamlRoot = XamlRoot,
                Title = "Modifier le classement ?",
                Content = "Les catégories retirées ne seront plus affichées. Les valeurs des champs qui restent applicables seront conservées ; les autres resteront dans l'historique, sans suppression définitive.",
                PrimaryButtonText = "Enregistrer",
                CloseButtonText = "Annuler",
                DefaultButton = ContentDialogButton.Close,
            };
            if (await dialog.ShowAsync() != ContentDialogResult.Primary) return;
        }
        await RunAsync(async () =>
        {
            await Api.ReplaceItemCategoriesAsync(item.Id, ids, item.Revision);
            await RefreshItemsCoreAsync();
            Items.SelectedItem = (Items.ItemsSource as IReadOnlyList<InventoryItem>)?
                .FirstOrDefault(candidate => candidate.Id == item.Id);
            ShowStatus("Classement enregistré.", InfoBarSeverity.Success);
        });
    }

    private void OnEffectiveFieldSelected(object sender, SelectionChangedEventArgs e)
    {
        var field = EffectiveFields.SelectedItem as EffectiveItemField;
        FieldValue.Text = field?.Value ?? string.Empty;
        FieldValue.PlaceholderText = field?.ValueType switch
        {
            "date" => "AAAA-MM-JJ",
            "number" => "Nombre décimal (point)",
            _ => "Texte",
        };
        UpdateButtons();
    }

    private async void OnSaveFieldValue(object sender, RoutedEventArgs e)
    {
        if (ActiveItem is not { } item || EffectiveFields.SelectedItem is not EffectiveItemField field) return;
        await RunAsync(async () =>
        {
            await Api.SetItemFieldValueAsync(item.Id, field.Id, FieldValue.Text.Trim(), item.Revision);
            await RefreshItemsCoreAsync();
            Items.SelectedItem = (Items.ItemsSource as IReadOnlyList<InventoryItem>)?
                .FirstOrDefault(candidate => candidate.Id == item.Id);
            ShowStatus("Valeur du champ enregistrée.", InfoBarSeverity.Success);
        });
    }

    private async void OnDestinationSelected(object sender, SelectionChangedEventArgs e)
    {
        if (DestinationSpaces.SelectedItem is not CollectionSpace destination)
        { DestinationCategoryChoices.ItemsSource = null; return; }
        await RunAsync(async () =>
        {
            var categories = await Api.GetCategoriesAsync(destination.Id);
            if ((DestinationSpaces.SelectedItem as CollectionSpace)?.Id == destination.Id)
                DestinationCategoryChoices.ItemsSource = CategoryOptions(categories);
        });
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
        RefreshStateAuditButton.IsEnabled = false;
        SaveItemNameButton.IsEnabled = false;
        ArchiveItemButton.IsEnabled = false;
        TrashItemButton.IsEnabled = false;
        RestoreItemButton.IsEnabled = false;
        InviteButton.IsEnabled = false;
        RefreshInvitationsButton.IsEnabled = false;
        SaveMemberRightsButton.IsEnabled = false;
        RemoveMemberButton.IsEnabled = false;
        RefreshMembersButton.IsEnabled = false;
        CreateCategoryButton.IsEnabled = false;
        CreateFieldButton.IsEnabled = false;
        SaveItemCategoriesButton.IsEnabled = false;
        SaveFieldValueButton.IsEnabled = false;
        CreateLocationButton.IsEnabled = false;
        MoveItemLocationButton.IsEnabled = false;
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
        RefreshStateAuditButton.IsEnabled = Api.IsSignedIn && ActiveItem is not null && CanReadStateAudit;
        SaveItemNameButton.IsEnabled = Api.IsSignedIn && ActiveItem is not null;
        var activeItemState = ActiveItem?.State ?? "active";
        ArchiveItemButton.IsEnabled = Api.IsSignedIn && ActiveItem is not null && activeItemState == "active";
        TrashItemButton.IsEnabled = Api.IsSignedIn && ActiveItem is not null && activeItemState != "trashed";
        RestoreItemButton.IsEnabled = Api.IsSignedIn && ActiveItem is not null && activeItemState != "active";
        InviteButton.IsEnabled = SharingPanel.Visibility == Visibility.Visible;
        RefreshInvitationsButton.IsEnabled = SharingPanel.Visibility == Visibility.Visible;
        RefreshMembersButton.IsEnabled = MembersPanel.Visibility == Visibility.Visible;
        var canChangeMember = MembersPanel.Visibility == Visibility.Visible &&
            Members.SelectedItem is SpaceMember member && member.AccountId != Api.CurrentAccountId;
        SaveMemberRightsButton.IsEnabled = canChangeMember;
        RemoveMemberButton.IsEnabled = canChangeMember &&
            (Members.SelectedItem as SpaceMember)?.AccountId != ActiveSpace?.OwnerAccountId;
        CreateCategoryButton.IsEnabled = Api.IsSignedIn && ActiveSpace is not null && _canReadActiveSpace;
        CreateFieldButton.IsEnabled = CreateCategoryButton.IsEnabled && FieldCategory.SelectedItem is CategoryOption { Id: not null };
        SaveItemCategoriesButton.IsEnabled = Api.IsSignedIn && ActiveItem is not null;
        SaveFieldValueButton.IsEnabled = Api.IsSignedIn && ActiveItem is not null && EffectiveFields.SelectedItem is not null;
        CreateLocationButton.IsEnabled = Api.IsSignedIn && ActiveSpace is not null && _canReadActiveSpace;
        MoveItemLocationButton.IsEnabled = Api.IsSignedIn && ActiveItem is not null && ItemLocationChoices.SelectedItem is LocationOption;
    }

    private void ShowStatus(string message, InfoBarSeverity severity)
    {
        StatusBar.Message = message;
        StatusBar.Severity = severity;
        StatusBar.IsOpen = true;
    }
}

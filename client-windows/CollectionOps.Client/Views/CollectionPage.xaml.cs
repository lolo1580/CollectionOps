using CollectionOps.Client.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;

namespace CollectionOps.Client.Views;

public sealed partial class CollectionPage : Page
{
    private static Guid? _lastSpaceId;
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
    private bool _canReadAcquisitions;
    private bool _canWriteAcquisitions;
    private bool _loadingWishes;
    private bool _loadingOffers;
    private bool _viewInitialized;
    private bool _changingFilters;
    private IReadOnlyList<AcquisitionVendor> _vendors = [];
    private bool _loadingCategories;
    private Guid? _categoryFilter;
    private Guid? _locationFilter;
    private Guid? _groupFilter;
    private bool _loadingGroups;
    private bool _loadingLocations;
    private IReadOnlyList<CollectionCategory> _categories = [];
    private IReadOnlyList<CollectionLocation> _locations = [];
    private IReadOnlyList<CollectionGroup> _groups = [];
    private int _assignedCategoryCount;
    private HashSet<Guid> _assignedCategoryIds = [];
    private IReadOnlyList<ItemRelation> _relations = [];

    public CollectionPage()
    {
        InitializeComponent();
        _viewInitialized = true;
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
        // The default ComboBoxItem is selected during InitializeComponent. Its event can
        // run before the rest of the page's named controls have been created.
        if (!_viewInitialized || _changingFilters || !Api.IsSignedIn) return;
        _stateFilter = (StateFilter.SelectedItem as ComboBoxItem)?.Tag as string ?? "active";
        await RefreshItemsAsync();
    }

    protected override void OnNavigatedTo(NavigationEventArgs e)
    {
        base.OnNavigatedTo(e);
        var section = e.Parameter as string ?? "inventory";
        InventorySection.Visibility = section == "inventory" ? Visibility.Visible : Visibility.Collapsed;
        AcquisitionsSection.Visibility = section == "acquisitions" ? Visibility.Visible : Visibility.Collapsed;
        OrganizationSection.Visibility = section == "organization" ? Visibility.Visible : Visibility.Collapsed;
        SharingSection.Visibility = section == "sharing" ? Visibility.Visible : Visibility.Collapsed;
        (SectionTitle.Text, SectionIntro.Text) = section switch
        {
            "acquisitions" => ("Acquisitions", "Suivez les objets recherchés, les vendeurs et leurs offres."),
            "organization" => ("Organisation", "Définissez catégories, champs, emplacements et regroupements pour cet espace."),
            "sharing" => ("Partage", "Gérez les invitations et les droits des membres de cet espace."),
            _ => ("Inventaire", "Retrouvez vos objets et ouvrez leur fiche."),
        };
    }

    private async void OnCategoryFilterChanged(object sender, SelectionChangedEventArgs e)
    {
        if (_loadingCategories || _changingFilters || !Api.IsSignedIn) return;
        _categoryFilter = (CategoryFilter.SelectedItem as CategoryOption)?.Id;
        await RefreshItemsAsync();
    }

    private async void OnLocationFilterChanged(object sender, SelectionChangedEventArgs e)
    {
        if (_loadingLocations || _changingFilters || !Api.IsSignedIn) return;
        _locationFilter = (LocationFilter.SelectedItem as LocationOption)?.Id;
        await RefreshItemsAsync();
    }

    private async void OnGroupFilterChanged(object sender, SelectionChangedEventArgs e)
    {
        if (_loadingGroups || _changingFilters || !Api.IsSignedIn) return;
        _groupFilter = (GroupFilter.SelectedItem as GroupOption)?.Id;
        await RefreshItemsAsync();
    }

    private async void OnLoadMore(object sender, RoutedEventArgs e)
    {
        if (ActiveSpace is not { } space || _nextCursor is not { } cursor) return;
        var search = _search;
        var state = _stateFilter;
        var categoryId = _categoryFilter;
        var locationId = _locationFilter;
        var groupId = _groupFilter;
        await RunAsync(async () =>
        {
            var page = await Api.GetItemsPageAsync(space.Id, search, cursor, state: state,
                categoryId: categoryId, locationId: locationId, groupId: groupId);
            if (ActiveSpace?.Id != space.Id || _search != search || _stateFilter != state ||
                _categoryFilter != categoryId || _locationFilter != locationId || _groupFilter != groupId) return;
            var items = (Items.ItemsSource as IEnumerable<InventoryItem>)?.ToList() ?? [];
            var selectedId = ActiveItem?.Id;
            var seenIds = items.Select(item => item.Id).ToHashSet();
            items.AddRange(page.Items.Where(item => seenIds.Add(item.Id)));
            Items.ItemsSource = items;
            Items.SelectedItem = items.FirstOrDefault(item => item.Id == selectedId);
            if (Items.SelectedItem is not null) Items.ScrollIntoView(Items.SelectedItem);
            _nextCursor = page.NextCursor;
        });
    }
    private async void OnSpaceSelected(object sender, SelectionChangedEventArgs e)
    {
        if (_loadingSpaces) return;
        _lastSpaceId = ActiveSpace?.Id;
        _categoryFilter = null;
        _locationFilter = null;
        _groupFilter = null;
        await RunAsync(async () =>
        {
            UpdateDestinationOptions();
            TransferHistory.ItemsSource = null;
            await RefreshMembersCoreAsync();
            await RefreshMyPermissionsCoreAsync();
            await RefreshCategoriesCoreAsync();
            await RefreshLocationsCoreAsync();
            await RefreshGroupsCoreAsync();
            await RefreshItemsCoreAsync();
            await RefreshAcquisitionsCoreAsync();
            await RefreshInvitationsCoreAsync();
        });
    }

    private async void OnItemSelected(object sender, SelectionChangedEventArgs e)
    {
        if (!_viewInitialized) return;
        ObjectDetailsPanel.Visibility = ActiveItem is null ? Visibility.Collapsed : Visibility.Visible;
        SelectedItemName.Text = ActiveItem is { } item
            ? $"{item.Name} — n° {item.InventoryNumber} (révision {item.Revision}, {item.StateLabel})"
            : "Sélectionnez un objet dans l'inventaire.";
        SelectedItemDetails.Text = ActiveItem is { } selected
            ? $"Identifiant : {selected.Id} · Créé le {selected.CreatedAt.ToLocalTime():g}"
            : string.Empty;
        EditedItemName.Text = ActiveItem?.Name ?? string.Empty;
        EditedDescription.Text = ActiveItem?.Description ?? string.Empty;
        EditedHistoricalReference.Text = ActiveItem?.HistoricalReference ?? string.Empty;
        EditedTechnicalReference.Text = ActiveItem?.TechnicalReference ?? string.Empty;
        ItemGroupChoices.SelectedItems.Clear();
        TransferHistory.ItemsSource = null;
        StateAuditHistory.ItemsSource = null;
        AssignedCategories.Text = "Aucune catégorie.";
        EffectiveFields.ItemsSource = null;
        CurrentLocationText.Text = "Aucun emplacement.";
        LocationHistory.ItemsSource = null;
        ItemRelations.ItemsSource = null;
        _relations = [];
        RelationTargetChoices.ItemsSource = null;
        _assignedCategoryCount = 0;
        _assignedCategoryIds.Clear();
        StateAuditPanel.Visibility = ActiveItem is not null && CanReadStateAudit
            ? Visibility.Visible : Visibility.Collapsed;
        UpdateButtons();
        if (ActiveItem is not null)
        {
            await RefreshHistoryAsync();
            await RefreshItemTaxonomyAsync();
            await RefreshItemGroupsAsync();
            await RefreshItemRelationsAsync();
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
        if (ActiveSpace is not { } space)
        {
            ShowStatus("Choisissez d'abord un espace.", InfoBarSeverity.Warning);
            return;
        }
        var nameBox = new TextBox { Header = "Nom de l'objet", MaxLength = 255, PlaceholderText = "Ex. : Maquette de char" };
        var dialog = new ContentDialog
        {
            XamlRoot = XamlRoot,
            Title = "Ajouter un objet à l'inventaire",
            Content = nameBox,
            PrimaryButtonText = "Ajouter",
            CloseButtonText = "Annuler",
            DefaultButton = ContentDialogButton.Primary,
            IsPrimaryButtonEnabled = false,
        };
        nameBox.TextChanged += (_, _) => dialog.IsPrimaryButtonEnabled = !string.IsNullOrWhiteSpace(nameBox.Text);
        if (await dialog.ShowAsync() != ContentDialogResult.Primary) return;
        var name = nameBox.Text.Trim();
        await RunAsync(async () =>
        {
            if (ActiveSpace?.Id != space.Id) return;
            var duplicate = await FindItemWithExactNameAsync(space.Id, name);
            if (duplicate is not null)
            {
                var confirmation = new ContentDialog
                {
                    XamlRoot = XamlRoot,
                    Title = "Un objet porte déjà ce nom",
                    Content = $"{duplicate.Name} (n° {duplicate.InventoryNumber}, {duplicate.StateLabel}) existe déjà dans cet espace. Voulez-vous vraiment créer un autre objet nommé « {name} » ?",
                    PrimaryButtonText = "Créer quand même",
                    CloseButtonText = "Annuler",
                    DefaultButton = ContentDialogButton.Close,
                };
                if (await confirmation.ShowAsync() != ContentDialogResult.Primary) return;
            }
            if (ActiveSpace?.Id != space.Id) return;
            var created = await Api.CreateItemAsync(space.Id, name);
            _changingFilters = true;
            try
            {
                _search = string.Empty;
                SearchText.Text = string.Empty;
                _stateFilter = "active";
                StateFilter.SelectedIndex = 0;
                _categoryFilter = null;
                CategoryFilter.SelectedIndex = 0;
                _locationFilter = null;
                LocationFilter.SelectedIndex = 0;
                _groupFilter = null;
                GroupFilter.SelectedIndex = 0;
            }
            finally { _changingFilters = false; }
            await RefreshItemsCoreAsync(created.Id, selectFirstWhenMissing: false);
            var visibleItems = (Items.ItemsSource as IEnumerable<InventoryItem>)?.ToList() ?? [];
            var selected = visibleItems.FirstOrDefault(candidate => candidate.Id == created.Id);
            if (selected is null)
            {
                visibleItems.Insert(0, created);
                Items.ItemsSource = visibleItems;
                Items.Visibility = Visibility.Visible;
                InventoryEmptyState.Visibility = Visibility.Collapsed;
                selected = created;
            }
            Items.SelectedItem = selected;
            Items.ScrollIntoView(selected);
            ShowStatus("Objet ajouté ; sa fiche est ouverte.", InfoBarSeverity.Success);
        });
    }

    private async Task<InventoryItem?> FindItemWithExactNameAsync(Guid spaceId, string name)
    {
        string? cursor = null;
        do
        {
            var page = await Api.GetItemsPageAsync(spaceId, name, cursor, limit: 100, state: "all");
            var duplicate = page.Items.FirstOrDefault(item =>
                string.Equals(item.Name.Trim(), name, StringComparison.OrdinalIgnoreCase));
            if (duplicate is not null) return duplicate;
            cursor = page.NextCursor;
        } while (cursor is not null);
        return null;
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

    private async void OnTransferOwnership(object sender, RoutedEventArgs e)
    {
        if (ActiveSpace is not { } space || Members.SelectedItem is not SpaceMember member) return;
        if (space.OwnerAccountId != Api.CurrentAccountId)
        {
            ShowStatus("Seul le propriétaire actuel peut transférer la propriété.", InfoBarSeverity.Warning);
            return;
        }
        if (member.AccountId == Api.CurrentAccountId)
        {
            ShowStatus("Choisissez un autre membre comme nouveau propriétaire.", InfoBarSeverity.Warning);
            return;
        }
        var dialog = new ContentDialog
        {
            XamlRoot = XamlRoot,
            Title = $"Transférer la propriété à {member.Label} ?",
            Content = "Le nouveau propriétaire pourra inviter et gérer les membres. Vous resterez membre avec vos droits actuels et ne pourrez plus transférer la propriété.",
            PrimaryButtonText = "Transférer",
            CloseButtonText = "Annuler",
            DefaultButton = ContentDialogButton.Close,
        };
        if (await dialog.ShowAsync() != ContentDialogResult.Primary) return;
        await RunAsync(async () =>
        {
            await Api.TransferSpaceOwnershipAsync(space.Id, member.AccountId);
            await LoadSpacesAsync(space.Id);
            ShowStatus("Propriété de l'espace transférée.", InfoBarSeverity.Success);
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

    private async Task RefreshMyPermissionsCoreAsync()
    {
        IReadOnlyList<string> permissions = [];
        if (ActiveSpace is { } space &&
            (!Api.CurrentIsSystemAdmin ||
             (Members.ItemsSource as IEnumerable<SpaceMember>)?.Any(member => member.AccountId == Api.CurrentAccountId) == true))
        {
            permissions = await Api.GetMySpacePermissionsAsync(space.Id);
        }
        _canReadActiveSpace = permissions.Contains("collections_read");
        _canReadAcquisitions = permissions.Contains("acquisitions_read");
        _canWriteAcquisitions = permissions.Contains("acquisitions_write");
        AcquisitionsPanel.Visibility = _canReadAcquisitions ? Visibility.Visible : Visibility.Collapsed;
        AcquisitionsNoAccess.Visibility = _canReadAcquisitions ? Visibility.Collapsed : Visibility.Visible;
        UpdateButtons();
    }

    private async Task RefreshAcquisitionsCoreAsync(Guid? selectWishId = null, Guid? selectVendorId = null)
    {
        if (ActiveSpace is not { } space || !_canReadAcquisitions)
        {
            Wishes.ItemsSource = null;
            Vendors.ItemsSource = null;
            OfferVendorChoices.ItemsSource = null;
            EditOfferVendorChoices.ItemsSource = null;
            WishOffers.ItemsSource = null;
            _vendors = [];
            return;
        }
        var previousWishId = selectWishId ?? (Wishes.SelectedItem as WishEntry)?.Id;
        var previousVendorId = selectVendorId ?? (Vendors.SelectedItem as AcquisitionVendor)?.Id;
        var previousOfferVendorId = (OfferVendorChoices.SelectedItem as AcquisitionVendor)?.Id;
        _vendors = await Api.GetVendorsAsync(space.Id);
        Vendors.ItemsSource = _vendors;
        Vendors.SelectedItem = _vendors.FirstOrDefault(vendor => vendor.Id == previousVendorId)
            ?? _vendors.FirstOrDefault();
        OfferVendorChoices.ItemsSource = _vendors;
        OfferVendorChoices.SelectedItem = _vendors.FirstOrDefault(vendor => vendor.Id == previousOfferVendorId)
            ?? _vendors.FirstOrDefault();
        EditOfferVendorChoices.ItemsSource = _vendors;
        var wishes = await Api.GetWishesAsync(space.Id);
        _loadingWishes = true;
        try
        {
            Wishes.ItemsSource = wishes;
            Wishes.SelectedItem = wishes.FirstOrDefault(wish => wish.Id == previousWishId)
                ?? wishes.FirstOrDefault();
        }
        finally { _loadingWishes = false; }
        PopulateWishEditor();
        await RefreshOffersCoreAsync();
        UpdateButtons();
    }

    private async Task RefreshOffersCoreAsync(Guid? selectOfferId = null)
    {
        if (ActiveSpace is not { } space || Wishes.SelectedItem is not WishEntry wish)
        {
            SelectedWishNotes.Text = "Choisissez une envie pour voir ses offres.";
            WishOffers.ItemsSource = null;
            PopulateOfferEditor();
            return;
        }
        var previousOfferId = selectOfferId ?? (WishOffers.SelectedItem as OfferLine)?.Offer.Id;
        SelectedWishNotes.Text = wish.SearchNotes ?? "Aucune note de recherche.";
        var offers = await Api.GetOffersAsync(space.Id, wish.Id);
        if (ActiveSpace?.Id != space.Id || (Wishes.SelectedItem as WishEntry)?.Id != wish.Id) return;
        var lines = offers.Select(offer => new OfferLine(offer,
            $"{offer.Title} — {_vendors.FirstOrDefault(vendor => vendor.Id == offer.VendorId)?.Name ?? "Fournisseur inconnu"}" +
            (offer.SourceUrl is null ? string.Empty : $" · {offer.SourceUrl}") +
            (offer.Notes is null ? string.Empty : $" · {offer.Notes}"))).ToList();
        _loadingOffers = true;
        try
        {
            WishOffers.ItemsSource = lines;
            WishOffers.SelectedItem = lines.FirstOrDefault(line => line.Offer.Id == previousOfferId)
                ?? lines.FirstOrDefault();
        }
        finally { _loadingOffers = false; }
        PopulateOfferEditor();
        UpdateButtons();
    }

    private void PopulateWishEditor()
    {
        var wish = Wishes.SelectedItem as WishEntry;
        EditWishTitle.Text = wish?.Title ?? string.Empty;
        EditWishNotes.Text = wish?.SearchNotes ?? string.Empty;
    }

    private void PopulateVendorEditor()
    {
        var vendor = Vendors.SelectedItem as AcquisitionVendor;
        EditVendorName.Text = vendor?.Name ?? string.Empty;
        EditVendorWebsite.Text = vendor?.WebsiteUrl ?? string.Empty;
        UpdateButtons();
    }

    private void PopulateOfferEditor()
    {
        var offer = (WishOffers.SelectedItem as OfferLine)?.Offer;
        EditOfferTitle.Text = offer?.Title ?? string.Empty;
        EditOfferUrl.Text = offer?.SourceUrl ?? string.Empty;
        EditOfferNotes.Text = offer?.Notes ?? string.Empty;
        EditOfferVendorChoices.SelectedItem = _vendors.FirstOrDefault(vendor => vendor.Id == offer?.VendorId);
        UpdateButtons();
    }

    private async void OnWishSelected(object sender, SelectionChangedEventArgs e)
    {
        if (_loadingWishes) return;
        PopulateWishEditor();
        WishOffers.ItemsSource = null;
        UpdateButtons();
        await RunAsync(() => RefreshOffersCoreAsync());
    }

    private void OnVendorSelected(object sender, SelectionChangedEventArgs e) => PopulateVendorEditor();

    private void OnOfferSelected(object sender, SelectionChangedEventArgs e)
    {
        if (!_loadingOffers) PopulateOfferEditor();
    }

    private void OnOfferVendorSelected(object sender, SelectionChangedEventArgs e) => UpdateButtons();

    private async void OnRefreshAcquisitions(object sender, RoutedEventArgs e) =>
        await RunAsync(() => RefreshAcquisitionsCoreAsync());

    private async void OnCreateWish(object sender, RoutedEventArgs e)
    {
        if (ActiveSpace is not { } space || string.IsNullOrWhiteSpace(NewWishTitle.Text)) return;
        await RunAsync(async () =>
        {
            var created = await Api.CreateWishAsync(space.Id, NewWishTitle.Text.Trim(), NewWishNotes.Text.Trim());
            NewWishTitle.Text = string.Empty;
            NewWishNotes.Text = string.Empty;
            await RefreshAcquisitionsCoreAsync(selectWishId: created.Id);
            ShowStatus("Envie ajoutée.", InfoBarSeverity.Success);
        });
    }

    private async void OnCreateVendor(object sender, RoutedEventArgs e)
    {
        if (ActiveSpace is not { } space || string.IsNullOrWhiteSpace(NewVendorName.Text)) return;
        await RunAsync(async () =>
        {
            var created = await Api.CreateVendorAsync(space.Id, NewVendorName.Text.Trim(), NewVendorWebsite.Text.Trim());
            NewVendorName.Text = string.Empty;
            NewVendorWebsite.Text = string.Empty;
            await RefreshAcquisitionsCoreAsync(selectVendorId: created.Id);
            OfferVendorChoices.SelectedItem = _vendors.FirstOrDefault(vendor => vendor.Id == created.Id);
            ShowStatus("Fournisseur ajouté.", InfoBarSeverity.Success);
        });
    }

    private async void OnCreateOffer(object sender, RoutedEventArgs e)
    {
        if (ActiveSpace is not { } space || Wishes.SelectedItem is not WishEntry wish ||
            OfferVendorChoices.SelectedItem is not AcquisitionVendor vendor ||
            string.IsNullOrWhiteSpace(NewOfferTitle.Text)) return;
        await RunAsync(async () =>
        {
            var created = await Api.CreateOfferAsync(space.Id, wish.Id, vendor.Id, NewOfferTitle.Text.Trim(),
                NewOfferUrl.Text.Trim(), NewOfferNotes.Text.Trim());
            NewOfferTitle.Text = string.Empty;
            NewOfferUrl.Text = string.Empty;
            NewOfferNotes.Text = string.Empty;
            await RefreshOffersCoreAsync(created.Id);
            ShowStatus("Offre ajoutée.", InfoBarSeverity.Success);
        });
    }

    private async void OnSaveWish(object sender, RoutedEventArgs e)
    {
        if (ActiveSpace is not { } space || Wishes.SelectedItem is not WishEntry wish ||
            string.IsNullOrWhiteSpace(EditWishTitle.Text)) return;
        await RunAsync(async () =>
        {
            await Api.UpdateWishAsync(space.Id, wish.Id, EditWishTitle.Text.Trim(),
                EditWishNotes.Text.Trim(), wish.Revision);
            await RefreshAcquisitionsCoreAsync(selectWishId: wish.Id);
            ShowStatus("Envie modifiée.", InfoBarSeverity.Success);
        });
    }

    private async void OnSaveVendor(object sender, RoutedEventArgs e)
    {
        if (ActiveSpace is not { } space || Vendors.SelectedItem is not AcquisitionVendor vendor ||
            string.IsNullOrWhiteSpace(EditVendorName.Text)) return;
        await RunAsync(async () =>
        {
            await Api.UpdateVendorAsync(space.Id, vendor.Id, EditVendorName.Text.Trim(),
                EditVendorWebsite.Text.Trim(), vendor.Revision);
            await RefreshAcquisitionsCoreAsync(selectVendorId: vendor.Id);
            ShowStatus("Fournisseur modifié.", InfoBarSeverity.Success);
        });
    }

    private async void OnSaveOffer(object sender, RoutedEventArgs e)
    {
        if (ActiveSpace is not { } space || Wishes.SelectedItem is not WishEntry wish ||
            WishOffers.SelectedItem is not OfferLine line ||
            EditOfferVendorChoices.SelectedItem is not AcquisitionVendor vendor ||
            string.IsNullOrWhiteSpace(EditOfferTitle.Text)) return;
        await RunAsync(async () =>
        {
            await Api.UpdateOfferAsync(space.Id, wish.Id, line.Offer.Id, vendor.Id,
                EditOfferTitle.Text.Trim(), EditOfferUrl.Text.Trim(), EditOfferNotes.Text.Trim(),
                line.Offer.Revision);
            await RefreshOffersCoreAsync(line.Offer.Id);
            ShowStatus("Offre modifiée.", InfoBarSeverity.Success);
        });
    }

    private Task RefreshSpacesAsync() => RunAsync(() => LoadSpacesAsync(ActiveSpace?.Id ?? _lastSpaceId));

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
        _lastSpaceId = ActiveSpace?.Id;
        if (previousSpaceId != ActiveSpace?.Id)
        {
            _categoryFilter = null;
            _locationFilter = null;
            _groupFilter = null;
        }
        UpdateDestinationOptions();
        TransferHistory.ItemsSource = null;
        await RefreshMembersCoreAsync();
        await RefreshMyPermissionsCoreAsync();
        await RefreshCategoriesCoreAsync();
        await RefreshLocationsCoreAsync();
        await RefreshGroupsCoreAsync();
        if (_spaces.Count == 0)
        {
            Items.ItemsSource = null;
            Items.Visibility = Visibility.Collapsed;
            InventoryEmptyState.Text = "Créez un espace pour commencer votre collection.";
            InventoryEmptyState.Visibility = Visibility.Visible;
            ShowStatus("Aucun espace visible. Créez votre premier espace.", InfoBarSeverity.Informational);
        }
        else await RefreshItemsCoreAsync();
        await RefreshAcquisitionsCoreAsync();
        await RefreshInvitationsCoreAsync();
        UpdateButtons();
    }

    private Task RefreshItemsAsync() => RunAsync(() => RefreshItemsCoreAsync());

    private async Task RefreshItemsCoreAsync(Guid? preferredItemId = null, bool selectFirstWhenMissing = true)
    {
        var selectedId = preferredItemId ?? ActiveItem?.Id;
        _nextCursor = null;
        if (ActiveSpace is { } space && _canReadActiveSpace)
        {
            var page = await Api.GetItemsPageAsync(space.Id, _search, state: _stateFilter,
                categoryId: _categoryFilter, locationId: _locationFilter, groupId: _groupFilter);
            Items.ItemsSource = page.Items;
            Items.Visibility = page.Items.Count == 0 ? Visibility.Collapsed : Visibility.Visible;
            _nextCursor = page.NextCursor;
            InventoryEmptyState.Text = _search.Length > 0 || _categoryFilter is not null ||
                _locationFilter is not null || _groupFilter is not null || _stateFilter != "active"
                ? "Aucun objet ne correspond à ces filtres. Essayez de les modifier."
                : "Aucun objet dans cet espace. Cliquez sur Ajouter un objet pour commencer.";
            InventoryEmptyState.Visibility = page.Items.Count == 0 ? Visibility.Visible : Visibility.Collapsed;
            Items.SelectedItem = page.Items.FirstOrDefault(item => item.Id == selectedId)
                ?? (selectFirstWhenMissing ? page.Items.FirstOrDefault() : null);
            if (Items.SelectedItem is not null) Items.ScrollIntoView(Items.SelectedItem);
        }
        else
        {
            Items.ItemsSource = null;
            Items.Visibility = Visibility.Collapsed;
            InventoryEmptyState.Text = ActiveSpace is null
                ? "Choisissez ou créez un espace pour commencer."
                : "Vous n'avez pas accès à l'inventaire de cet espace.";
            InventoryEmptyState.Visibility = Visibility.Visible;
            Items.SelectedItem = null;
        }
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

    private async void OnSaveItemDetails(object sender, RoutedEventArgs e)
    {
        if (ActiveItem is not { } item) return;
        await RunAsync(async () =>
        {
            var updated = await Api.UpdateItemDetailsAsync(item.Id, EditedDescription.Text,
                EditedHistoricalReference.Text, EditedTechnicalReference.Text, item.Revision);
            await RefreshItemsCoreAsync();
            Items.SelectedItem = (Items.ItemsSource as IReadOnlyList<InventoryItem>)?
                .FirstOrDefault(candidate => candidate.Id == updated.Id);
            ShowStatus("Description et références enregistrées.", InfoBarSeverity.Success);
        });
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
        var previousManagedId = (ManageCategories.SelectedItem as CategoryOption)?.Id;
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
        ManageCategories.ItemsSource = options;
        ManageCategories.SelectedItem = options.FirstOrDefault(option => option.Id == previousManagedId);
        if (ManageCategories.SelectedItem is null) EditedCategoryName.Text = string.Empty;
        CategoryFields.ItemsSource = null;
        UpdateButtons();
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

    private async Task RefreshGroupsCoreAsync()
    {
        var selectedId = (ManageGroups.SelectedItem as CollectionGroup)?.Id;
        _groups = ActiveSpace is { } space && _canReadActiveSpace
            ? await Api.GetGroupsAsync(space.Id) : [];
        var options = _groups.Select(group => new GroupOption(group.Id, group.Label)).ToList();
        _loadingGroups = true;
        try
        {
            GroupFilter.ItemsSource = new[] { new GroupOption(null, "Toutes les séries et regroupements") }
                .Concat(options).ToList();
            GroupFilter.SelectedItem = (GroupFilter.ItemsSource as IEnumerable<GroupOption>)?
                .FirstOrDefault(option => option.Id == _groupFilter);
        }
        finally { _loadingGroups = false; }
        ManageGroups.ItemsSource = _groups;
        ManageGroups.SelectedItem = _groups.FirstOrDefault(group => group.Id == selectedId);
        ItemGroupChoices.ItemsSource = options;
    }

    private void OnManageGroupSelected(object sender, SelectionChangedEventArgs e)
    {
        EditedGroupName.Text = (ManageGroups.SelectedItem as CollectionGroup)?.Name ?? string.Empty;
        UpdateButtons();
    }

    private async void OnRenameGroup(object sender, RoutedEventArgs e)
    {
        if (ActiveSpace is not { } space || ManageGroups.SelectedItem is not CollectionGroup group) return;
        var name = EditedGroupName.Text.Trim();
        if (name.Length == 0)
        {
            ShowStatus("Saisissez un nom.", InfoBarSeverity.Warning);
            return;
        }
        await RunAsync(async () =>
        {
            await Api.RenameGroupAsync(space.Id, group.Id, name, group.Revision);
            await RefreshGroupsCoreAsync();
            if (ActiveItem is not null) await RefreshItemGroupsCoreAsync();
            ShowStatus("Série ou regroupement renommé.", InfoBarSeverity.Success);
        });
    }

    private async void OnDeleteGroup(object sender, RoutedEventArgs e)
    {
        if (ActiveSpace is not { } space || ManageGroups.SelectedItem is not CollectionGroup group) return;
        var dialog = new ContentDialog
        {
            XamlRoot = XamlRoot,
            Title = $"Supprimer {group.Label} ?",
            Content = "Seul un regroupement sans objets peut être supprimé. Cette suppression ne peut pas être annulée.",
            PrimaryButtonText = "Supprimer",
            CloseButtonText = "Annuler",
            DefaultButton = ContentDialogButton.Close,
        };
        if (await dialog.ShowAsync() != ContentDialogResult.Primary) return;
        await RunAsync(async () =>
        {
            await Api.DeleteGroupAsync(space.Id, group.Id, group.Revision);
            if (_groupFilter == group.Id) _groupFilter = null;
            await RefreshGroupsCoreAsync();
            await RefreshItemsCoreAsync();
            ShowStatus("Série ou regroupement vide supprimé.", InfoBarSeverity.Success);
        });
    }

    private async void OnCreateGroup(object sender, RoutedEventArgs e)
    {
        if (ActiveSpace is not { } space || string.IsNullOrWhiteSpace(NewGroupName.Text)) return;
        var kind = (NewGroupKind.SelectedItem as ComboBoxItem)?.Tag as string ?? "series";
        await RunAsync(async () =>
        {
            await Api.CreateGroupAsync(space.Id, kind, NewGroupName.Text.Trim());
            NewGroupName.Text = string.Empty;
            await RefreshGroupsCoreAsync();
            if (ActiveItem is not null) await RefreshItemGroupsCoreAsync();
            ShowStatus("Série ou regroupement créé.", InfoBarSeverity.Success);
        });
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
        EditedFieldName.Text = string.Empty;
        if (ActiveSpace is not { } space || FieldCategory.SelectedItem is not CategoryOption { Id: { } id })
        {
            CategoryFields.ItemsSource = null;
            UpdateButtons();
            return;
        }
        await RunAsync(async () => CategoryFields.ItemsSource = await Api.GetCategoryFieldsAsync(space.Id, id));
    }

    private void OnManageCategorySelected(object sender, SelectionChangedEventArgs e)
    {
        var option = ManageCategories.SelectedItem as CategoryOption;
        EditedCategoryName.Text = option?.Id is { } id
            ? _categories.FirstOrDefault(category => category.Id == id)?.Name ?? string.Empty
            : string.Empty;
        UpdateButtons();
    }

    private async void OnRenameCategory(object sender, RoutedEventArgs e)
    {
        if (ActiveSpace is not { } space || ManageCategories.SelectedItem is not CategoryOption { Id: { } categoryId }) return;
        var name = EditedCategoryName.Text.Trim();
        if (name.Length == 0)
        {
            ShowStatus("Saisissez un nom de catégorie.", InfoBarSeverity.Warning);
            return;
        }
        await RunAsync(async () =>
        {
            await Api.RenameCategoryAsync(space.Id, categoryId, name);
            await RefreshCategoriesCoreAsync();
            ManageCategories.SelectedItem = (ManageCategories.ItemsSource as IEnumerable<CategoryOption>)?
                .FirstOrDefault(candidate => candidate.Id == categoryId);
            if (ActiveItem is not null) await RefreshItemTaxonomyCoreAsync();
            ShowStatus("Catégorie renommée.", InfoBarSeverity.Success);
        });
    }

    private void OnCategoryFieldSelected(object sender, SelectionChangedEventArgs e)
    {
        EditedFieldName.Text = (CategoryFields.SelectedItem as CategoryFieldDefinition)?.Name ?? string.Empty;
        UpdateButtons();
    }

    private async void OnRenameCategoryField(object sender, RoutedEventArgs e)
    {
        if (ActiveSpace is not { } space || FieldCategory.SelectedItem is not CategoryOption { Id: { } categoryId } ||
            CategoryFields.SelectedItem is not CategoryFieldDefinition field) return;
        var name = EditedFieldName.Text.Trim();
        if (name.Length == 0)
        {
            ShowStatus("Saisissez un nom de champ.", InfoBarSeverity.Warning);
            return;
        }
        await RunAsync(async () =>
        {
            await Api.RenameCategoryFieldAsync(space.Id, categoryId, field.Id, name);
            var fields = await Api.GetCategoryFieldsAsync(space.Id, categoryId);
            CategoryFields.ItemsSource = fields;
            CategoryFields.SelectedItem = fields.FirstOrDefault(candidate => candidate.Id == field.Id);
            if (ActiveItem is not null) await RefreshItemTaxonomyCoreAsync();
            ShowStatus("Champ renommé.", InfoBarSeverity.Success);
        });
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

    private Task RefreshItemGroupsAsync() => RunAsync(RefreshItemGroupsCoreAsync);

    private async Task RefreshItemGroupsCoreAsync()
    {
        if (ActiveItem is not { } item) return;
        var assigned = await Api.GetItemGroupsAsync(item.Id);
        if (ActiveItem?.Id != item.Id) return;
        var ids = assigned.Groups.Select(group => group.Id).ToHashSet();
        ItemGroupChoices.SelectedItems.Clear();
        foreach (var option in (ItemGroupChoices.ItemsSource as IEnumerable<GroupOption>) ?? [])
            if (option.Id is { } id && ids.Contains(id)) ItemGroupChoices.SelectedItems.Add(option);
    }

    private async void OnSaveItemGroups(object sender, RoutedEventArgs e)
    {
        if (ActiveItem is not { } item) return;
        var ids = ItemGroupChoices.SelectedItems.Cast<GroupOption>()
            .Where(option => option.Id.HasValue).Select(option => option.Id!.Value).ToList();
        await RunAsync(async () =>
        {
            await Api.ReplaceItemGroupsAsync(item.Id, ids, item.Revision);
            await RefreshItemsCoreAsync();
            Items.SelectedItem = (Items.ItemsSource as IReadOnlyList<InventoryItem>)?
                .FirstOrDefault(candidate => candidate.Id == item.Id);
            ShowStatus("Séries et regroupements enregistrés.", InfoBarSeverity.Success);
        });
    }

    private Task RefreshItemRelationsAsync() => RunAsync(RefreshItemRelationsCoreAsync);

    private async Task RefreshItemRelationsCoreAsync()
    {
        if (ActiveItem is not { } item) return;
        var relations = await Api.GetItemRelationsAsync(item.Id);
        if (ActiveItem?.Id != item.Id) return;
        _relations = relations.Relations;
        ItemRelations.ItemsSource = relations.Relations;
        await RefreshRelationTargetsCoreAsync(item);
    }

    private async Task RefreshRelationTargetsCoreAsync(InventoryItem item)
    {
        if (ActiveSpace is not { } space)
        {
            RelationTargetChoices.ItemsSource = null;
            return;
        }
        var page = await Api.GetItemsPageAsync(space.Id, limit: 100, state: "all");
        if (ActiveItem?.Id != item.Id) return;
        RelationTargetChoices.ItemsSource = page.Items
            .Where(candidate => candidate.Id != item.Id)
            .Select(candidate => new RelationOption(candidate.Id, $"n° {candidate.InventoryNumber} · {candidate.Name}"))
            .ToList();
    }

    private void OnRelationSelected(object sender, SelectionChangedEventArgs e) => UpdateButtons();

    private async void OnRefreshRelations(object sender, RoutedEventArgs e) => await RefreshItemRelationsAsync();

    private async void OnAddRelation(object sender, RoutedEventArgs e)
    {
        if (ActiveItem is not { } item || RelationTargetChoices.SelectedItem is not RelationOption target) return;
        var kind = (RelationKindChoices.SelectedItem as ComboBoxItem)?.Tag as string ?? "related";
        var outgoing = OutgoingRelations();
        if (outgoing.Any(entry => entry.TargetId == target.Id && entry.Kind == kind))
        {
            ShowStatus("Ce lien existe déjà.", InfoBarSeverity.Warning);
            return;
        }
        outgoing.Add(new ItemRelationInput(target.Id, kind));
        await ReplaceRelationsAsync(item, outgoing, "Lien ajouté.");
    }

    private async void OnRemoveRelation(object sender, RoutedEventArgs e)
    {
        if (ActiveItem is not { } item)
        {
            return;
        }
        if (ItemRelations.SelectedItem is not ItemRelation { Direction: "outgoing" } relation)
        {
            ShowStatus("Seul un lien sortant peut être retiré depuis cet objet.", InfoBarSeverity.Warning);
            return;
        }
        var outgoing = OutgoingRelations()
            .Where(entry => !(entry.TargetId == relation.RelatedItemId && entry.Kind == relation.Kind))
            .ToList();
        await ReplaceRelationsAsync(item, outgoing, "Lien retiré.");
    }

    private List<ItemRelationInput> OutgoingRelations() => _relations
        .Where(relation => relation.Direction == "outgoing")
        .Select(relation => new ItemRelationInput(relation.RelatedItemId, relation.Kind))
        .ToList();

    private Task ReplaceRelationsAsync(InventoryItem item, List<ItemRelationInput> relations, string success) =>
        RunAsync(async () =>
        {
            await Api.ReplaceItemRelationsAsync(item.Id, relations, item.Revision);
            await RefreshItemsCoreAsync();
            Items.SelectedItem = (Items.ItemsSource as IReadOnlyList<InventoryItem>)?
                .FirstOrDefault(candidate => candidate.Id == item.Id);
            ShowStatus(success, InfoBarSeverity.Success);
        });

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
        SaveItemDetailsButton.IsEnabled = false;
        SaveItemGroupsButton.IsEnabled = false;
        CreateGroupButton.IsEnabled = false;
        CreateWishButton.IsEnabled = false;
        CreateVendorButton.IsEnabled = false;
        CreateOfferButton.IsEnabled = false;
        RenameGroupButton.IsEnabled = false;
        DeleteGroupButton.IsEnabled = false;
        ArchiveItemButton.IsEnabled = false;
        TrashItemButton.IsEnabled = false;
        RestoreItemButton.IsEnabled = false;
        InviteButton.IsEnabled = false;
        RefreshInvitationsButton.IsEnabled = false;
        SaveMemberRightsButton.IsEnabled = false;
        RemoveMemberButton.IsEnabled = false;
        RefreshMembersButton.IsEnabled = false;
        TransferOwnershipButton.IsEnabled = false;
        CreateCategoryButton.IsEnabled = false;
        CreateFieldButton.IsEnabled = false;
        SaveItemCategoriesButton.IsEnabled = false;
        SaveFieldValueButton.IsEnabled = false;
        CreateLocationButton.IsEnabled = false;
        MoveItemLocationButton.IsEnabled = false;
        RenameCategoryButton.IsEnabled = false;
        RenameFieldButton.IsEnabled = false;
        AddRelationButton.IsEnabled = false;
        RemoveRelationButton.IsEnabled = false;
        RefreshRelationsButton.IsEnabled = false;
        try { await action(); }
        catch (Exception error) when (error is HttpRequestException or TaskCanceledException or ArgumentException or InvalidOperationException or System.Text.Json.JsonException or NotSupportedException)
        {
            ShowStatus(error is TaskCanceledException ? "Le serveur ne répond pas." : error.Message, InfoBarSeverity.Error);
        }
        finally { UpdateButtons(); }
    }

    private void UpdateButtons()
    {
        SharingNoAccess.Visibility = SharingPanel.Visibility == Visibility.Visible || MembersPanel.Visibility == Visibility.Visible
            ? Visibility.Collapsed : Visibility.Visible;
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
        SaveItemDetailsButton.IsEnabled = Api.IsSignedIn && ActiveItem is not null && ActiveItem.State != "trashed";
        SaveItemGroupsButton.IsEnabled = Api.IsSignedIn && ActiveItem is not null && ActiveItem.State != "trashed";
        CreateGroupButton.IsEnabled = Api.IsSignedIn && ActiveSpace is not null && _canReadActiveSpace;
        CreateWishButton.IsEnabled = Api.IsSignedIn && ActiveSpace is not null && _canWriteAcquisitions;
        RefreshAcquisitionsButton.IsEnabled = Api.IsSignedIn && ActiveSpace is not null && _canReadAcquisitions;
        SaveWishButton.IsEnabled = CreateWishButton.IsEnabled && Wishes.SelectedItem is WishEntry;
        CreateVendorButton.IsEnabled = CreateWishButton.IsEnabled;
        SaveVendorButton.IsEnabled = CreateVendorButton.IsEnabled && Vendors.SelectedItem is AcquisitionVendor;
        CreateOfferButton.IsEnabled = CreateWishButton.IsEnabled && Wishes.SelectedItem is WishEntry &&
            OfferVendorChoices.SelectedItem is AcquisitionVendor;
        SaveOfferButton.IsEnabled = CreateWishButton.IsEnabled && WishOffers.SelectedItem is OfferLine &&
            EditOfferVendorChoices.SelectedItem is AcquisitionVendor;
        RenameGroupButton.IsEnabled = CreateGroupButton.IsEnabled && ManageGroups.SelectedItem is CollectionGroup;
        DeleteGroupButton.IsEnabled = RenameGroupButton.IsEnabled;
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
        TransferOwnershipButton.IsEnabled = MembersPanel.Visibility == Visibility.Visible &&
            ActiveSpace?.OwnerAccountId == Api.CurrentAccountId &&
            Members.SelectedItem is SpaceMember newOwner && newOwner.AccountId != Api.CurrentAccountId;
        CreateCategoryButton.IsEnabled = Api.IsSignedIn && ActiveSpace is not null && _canReadActiveSpace;
        CreateFieldButton.IsEnabled = CreateCategoryButton.IsEnabled && FieldCategory.SelectedItem is CategoryOption { Id: not null };
        SaveItemCategoriesButton.IsEnabled = Api.IsSignedIn && ActiveItem is not null;
        SaveFieldValueButton.IsEnabled = Api.IsSignedIn && ActiveItem is not null && EffectiveFields.SelectedItem is not null;
        CreateLocationButton.IsEnabled = Api.IsSignedIn && ActiveSpace is not null && _canReadActiveSpace;
        MoveItemLocationButton.IsEnabled = Api.IsSignedIn && ActiveItem is not null && ItemLocationChoices.SelectedItem is LocationOption;
        RenameCategoryButton.IsEnabled = CreateCategoryButton.IsEnabled && ManageCategories.SelectedItem is CategoryOption { Id: not null };
        RenameFieldButton.IsEnabled = CreateFieldButton.IsEnabled && CategoryFields.SelectedItem is CategoryFieldDefinition;
        var canEditRelations = Api.IsSignedIn && ActiveItem is not null && ActiveItem.State != "trashed";
        RefreshRelationsButton.IsEnabled = Api.IsSignedIn && ActiveItem is not null;
        AddRelationButton.IsEnabled = canEditRelations && RelationTargetChoices.SelectedItem is RelationOption;
        RemoveRelationButton.IsEnabled = canEditRelations &&
            ItemRelations.SelectedItem is ItemRelation { Direction: "outgoing" };
    }

    private void ShowStatus(string message, InfoBarSeverity severity)
    {
        StatusBar.Message = message;
        StatusBar.Severity = severity;
        StatusBar.IsOpen = true;
    }
}

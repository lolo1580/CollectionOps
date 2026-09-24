using System.Net;
using System.Text;
using CollectionOps.Client.Services;

await RejectInsecureRemoteServer();
await AllowPrivateLanHttpOnlyWithOptIn();
await ChangingLanModeClearsSession();
await RejectUnexpectedHealth();
await CheckDatabaseReadiness();
await ClearExpiredSession();
await RejectMalformedJson();
await ReadCollectionWithSession();
await TransferUsesExpectedRevision();
await ReadTransferHistory();
await ReadStateAuditHistory();
await ReadInventoryPage();
await InventoryPageCarriesStateFilter();
await ArchiveUsesExpectedRevision();
await AcceptNewAccountInvitationWithoutLeakingTokenToUrl();
await AcceptExistingAccountInvitationWithSession();
await RejectInvitationForDifferentServer();
await AdminCanListSpacesWithoutInventoryAccess();
await MemberGrantUpdateSendsExplicitRights();
await ReadNestedCategories();
await AssignCategoriesAndWriteInheritedField();
await TransferWithDestinationCategories();
await ReplaceCategoriesAllowsEmptySelection();
await InventoryPageCarriesCategoryFilter();
await ReadLocationsAndMoveItem();
await UpdateItemDetailsUsesRevision();
await AssignGroupsAndFilterInventory();
await RenameAndDeleteEmptyGroup();
await ReadAcquisitionPermissionsAndWishes();
await CreateVendorAndOfferWithoutAmounts();
await RenameCategoryAndFieldDefinitions();
await ReadAndReplaceItemRelations();
await TransferSpaceOwnership();
await UpdateAcquisitionProspectsWithRevision();
Console.WriteLine("SessionApi: 34 checks passed.");

static Task RejectInsecureRemoteServer()
{
    using var api = new SessionApi(new FakeHandler());
    Expect<ArgumentException>(() => api.Configure("http://example.org"));
    Expect<ArgumentException>(() => api.Configure("http://192.168.30.60:8080"));
    api.Configure("http://127.0.0.1:8080");
    return Task.CompletedTask;
}

static Task AllowPrivateLanHttpOnlyWithOptIn()
{
    using var api = new SessionApi(new FakeHandler());
    api.Configure("http://192.168.30.60:8080", true);
    Assert(api.ServerAddress == "http://192.168.30.60:8080/" && api.AllowInsecurePrivateNetwork,
        "A private IPv4 development server must require explicit opt-in.");
    api.Configure("http://10.0.0.10:8080");
    api.Configure("http://172.16.0.10:8080");
    Expect<ArgumentException>(() => api.Configure("http://172.32.0.10:8080"));
    Expect<ArgumentException>(() => api.Configure("http://169.254.1.2:8080"));
    Expect<ArgumentException>(() => api.Configure("http://8.8.8.8:8080"));
    Expect<ArgumentException>(() => api.Configure("http://collectionops.example.org:8080"));
    return Task.CompletedTask;
}

static async Task ChangingLanModeClearsSession()
{
    var handler = new FakeHandler();
    handler.Enqueue(HttpStatusCode.Created, """{"token":"secret","session":{"id":"session-1"}}""");
    using var api = new SessionApi(handler);
    api.Configure("http://127.0.0.1:8080");
    await api.SignInAsync("root@example.org", "password");
    api.Configure("http://192.168.30.60:8080", true);
    Assert(!api.IsSignedIn, "Switching to the private LAN server must discard the previous token.");
    Expect<ArgumentException>(() => api.Configure("http://192.168.30.60:8080", false));
    Assert(api.AllowInsecurePrivateNetwork, "A rejected configuration must not change the saved transport mode.");
}

static async Task RejectUnexpectedHealth()
{
    var handler = new FakeHandler();
    handler.Enqueue(HttpStatusCode.OK, """{"status":"unexpected"}""");
    using var api = new SessionApi(handler);
    api.Configure("http://127.0.0.1:8080");
    await ExpectAsync<InvalidOperationException>(() => api.CheckHealthAsync());
}

static async Task CheckDatabaseReadiness()
{
    var handler = new FakeHandler();
    handler.Enqueue(HttpStatusCode.OK, """{"status":"ready"}""");
    using var api = new SessionApi(handler);
    api.Configure("http://127.0.0.1:8080");
    Assert(await api.CheckHealthAsync() == "Serveur disponible" &&
           handler.LastPath == "/api/v1/health/ready",
        "The client must check database readiness rather than process health.");
}

static async Task ClearExpiredSession()
{
    var handler = new FakeHandler();
    handler.Enqueue(HttpStatusCode.Created, """{"token":"secret","session":{"id":"session-1"}}""");
    handler.Enqueue(HttpStatusCode.Unauthorized, """{}""");
    using var api = new SessionApi(handler);
    api.Configure("http://127.0.0.1:8080");
    await api.SignInAsync("root@example.org", "password");
    Assert(api.IsSignedIn, "Login must store the session in memory.");
    await ExpectAsync<InvalidOperationException>(() => api.GetSessionsAsync());
    Assert(!api.IsSignedIn && api.CurrentSessionId is null, "A 401 must clear the expired session.");
    Assert(handler.LastToken == "secret", "The token must be sent in x-session-token.");
}

static async Task RejectMalformedJson()
{
    var handler = new FakeHandler();
    handler.Enqueue(HttpStatusCode.OK, "not-json");
    using var api = new SessionApi(handler);
    api.Configure("http://127.0.0.1:8080");
    await ExpectAsync<System.Text.Json.JsonException>(() => api.CheckHealthAsync());
}

static async Task ReadCollectionWithSession()
{
    var handler = new FakeHandler();
    handler.Enqueue(HttpStatusCode.Created, """{"token":"secret","session":{"id":"session-1"}}""");
    handler.Enqueue(HttpStatusCode.OK, """{"spaces":[{"id":"01900000-0000-7000-8000-000000000001","name":"Ma collection","owner_account_id":"01900000-0000-7000-8000-000000000002"}]}""");
    using var api = new SessionApi(handler);
    api.Configure("http://127.0.0.1:8080");
    await api.SignInAsync("root@example.org", "password");
    var spaces = await api.GetSpacesAsync();
    Assert(spaces.Count == 1 && spaces[0].Name == "Ma collection", "The authenticated collection list must parse.");
    Assert(handler.LastToken == "secret", "Collection calls must carry the session token.");
}

static async Task TransferUsesExpectedRevision()
{
    var handler = new FakeHandler();
    handler.Enqueue(HttpStatusCode.Created, """{"token":"secret","session":{"id":"session-1"}}""");
    handler.Enqueue(HttpStatusCode.OK, """{"item":{"id":"01900000-0000-7000-8000-000000000001","space_id":"01900000-0000-7000-8000-000000000002","inventory_number":"2","name":"Objet","revision":"2"},"transfer":{"id":"01900000-0000-7000-8000-000000000003","source_space_id":"01900000-0000-7000-8000-000000000004","destination_space_id":"01900000-0000-7000-8000-000000000002","source_inventory_number":"1","destination_inventory_number":"2","transferred_at":"2026-09-23T10:00:00Z"}}""");
    using var api = new SessionApi(handler);
    api.Configure("http://127.0.0.1:8080");
    await api.SignInAsync("root@example.org", "password");
    var itemId = Guid.Parse("01900000-0000-7000-8000-000000000001");
    var destinationId = Guid.Parse("01900000-0000-7000-8000-000000000002");
    var outcome = await api.TransferItemAsync(itemId, destinationId, "1");
    Assert(outcome.Item.Revision == "2" && outcome.Transfer.DestinationInventoryNumber == "2", "Transfer response must parse.");
    Assert(handler.LastPath == $"/api/v1/items/{itemId}/transfers", "Transfer route must target the selected item.");
    Assert(handler.LastBody?.Contains("\"expected_revision\":\"1\"") == true, "Transfer must send the displayed revision.");
    Assert(handler.LastToken == "secret", "Transfer must carry the session token.");
}

static async Task ReadTransferHistory()
{
    var handler = new FakeHandler();
    handler.Enqueue(HttpStatusCode.Created, """{"token":"secret","session":{"id":"session-1"}}""");
    handler.Enqueue(HttpStatusCode.OK, """{"transfers":[{"id":"01900000-0000-7000-8000-000000000003","source_space_id":"01900000-0000-7000-8000-000000000004","destination_space_id":"01900000-0000-7000-8000-000000000002","source_inventory_number":"1","destination_inventory_number":"2","transferred_at":"2026-09-23T10:00:00Z"}]}""");
    using var api = new SessionApi(handler);
    api.Configure("http://127.0.0.1:8080");
    await api.SignInAsync("root@example.org", "password");
    var itemId = Guid.Parse("01900000-0000-7000-8000-000000000001");
    var history = await api.GetItemTransfersAsync(itemId);
    Assert(history.Count == 1 && history[0].SourceInventoryNumber == "1", "History must parse transfer numbers.");
    Assert(handler.LastPath == $"/api/v1/items/{itemId}/transfers", "History route must target the selected item.");
}

static async Task ReadNestedCategories()
{
    var handler = new FakeHandler();
    var spaceId = Guid.NewGuid();
    var parentId = Guid.NewGuid();
    var childId = Guid.NewGuid();
    handler.Enqueue(HttpStatusCode.Created, """{"token":"secret","session":{"id":"session-1"}}""");
    handler.Enqueue(HttpStatusCode.OK,
        $"{{\"categories\":[{{\"id\":\"{parentId}\",\"space_id\":\"{spaceId}\",\"parent_id\":null,\"name\":\"Armes\"}},{{\"id\":\"{childId}\",\"space_id\":\"{spaceId}\",\"parent_id\":\"{parentId}\",\"name\":\"Casques\"}}]}}");
    using var api = new SessionApi(handler);
    api.Configure("http://127.0.0.1:8080");
    await api.SignInAsync("root@example.org", "password");
    var categories = await api.GetCategoriesAsync(spaceId);
    Assert(categories.Count == 2 && categories[1].ParentId == parentId,
        "Nested categories must preserve parent IDs.");
    Assert(handler.LastPath == $"/api/v1/spaces/{spaceId}/categories", "Category route must target the space.");
}

static async Task AssignCategoriesAndWriteInheritedField()
{
    var handler = new FakeHandler();
    var itemId = Guid.NewGuid();
    var categoryId = Guid.NewGuid();
    var fieldId = Guid.NewGuid();
    handler.Enqueue(HttpStatusCode.Created, """{"token":"secret","session":{"id":"session-1"}}""");
    handler.Enqueue(HttpStatusCode.OK,
        $"{{\"revision\":\"2\",\"categories\":[{{\"category_id\":\"{categoryId}\",\"category_name\":\"Casques\"}}]}}");
    handler.Enqueue(HttpStatusCode.OK,
        $"{{\"revision\":\"3\",\"fields\":[{{\"id\":\"{fieldId}\",\"defined_category_name\":\"Armes\",\"name\":\"Date\",\"value_type\":\"date\",\"value\":\"1944-06-06\"}}]}}");
    using var api = new SessionApi(handler);
    api.Configure("http://127.0.0.1:8080");
    await api.SignInAsync("root@example.org", "password");
    var assigned = await api.AddItemCategoriesAsync(itemId, [categoryId], "1");
    Assert(assigned.Categories.Single().CategoryId == categoryId &&
        handler.LastBody?.Contains("\"expected_revision\":\"1\"") == true,
        "Category assignment must send the chosen category and revision.");
    var fields = await api.SetItemFieldValueAsync(itemId, fieldId, "1944-06-06", "2");
    Assert(fields.Fields.Single().DefinedCategoryName == "Armes" &&
        handler.LastBody?.Contains("\"value\":\"1944-06-06\"") == true,
        "An inherited field value must round-trip through the client contract.");
}

static async Task TransferWithDestinationCategories()
{
    var handler = new FakeHandler();
    var itemId = Guid.NewGuid();
    var destinationId = Guid.NewGuid();
    var categoryId = Guid.NewGuid();
    handler.Enqueue(HttpStatusCode.Created, """{"token":"secret","session":{"id":"session-1"}}""");
    handler.Enqueue(HttpStatusCode.OK,
        $"{{\"item\":{{\"id\":\"{itemId}\",\"space_id\":\"{destinationId}\",\"inventory_number\":\"2\",\"name\":\"Objet\",\"revision\":\"2\"}},\"transfer\":{{\"id\":\"{Guid.NewGuid()}\",\"source_space_id\":\"{Guid.NewGuid()}\",\"destination_space_id\":\"{destinationId}\",\"source_inventory_number\":\"1\",\"destination_inventory_number\":\"2\",\"transferred_at\":\"2026-09-23T10:00:00Z\"}}}}");
    using var api = new SessionApi(handler);
    api.Configure("http://127.0.0.1:8080");
    await api.SignInAsync("root@example.org", "password");
    await api.TransferItemAsync(itemId, destinationId, "1", [categoryId]);
    Assert(handler.LastBody?.Contains($"\"destination_category_ids\":[\"{categoryId}\"]") == true,
        "Transfer must send explicit destination classification.");
}

static async Task ReplaceCategoriesAllowsEmptySelection()
{
    var handler = new FakeHandler();
    var itemId = Guid.NewGuid();
    handler.Enqueue(HttpStatusCode.Created, """{"token":"secret","session":{"id":"session-1"}}""");
    handler.Enqueue(HttpStatusCode.OK, """{"revision":"7","categories":[]}""");
    using var api = new SessionApi(handler);
    api.Configure("http://127.0.0.1:8080");
    await api.SignInAsync("root@example.org", "password");
    var result = await api.ReplaceItemCategoriesAsync(itemId, [], "6");
    Assert(result.Revision == "7" && result.Categories.Count == 0,
        "Replacing categories must allow an empty active classification.");
    Assert(handler.LastMethod == HttpMethod.Put && handler.LastPath == $"/api/v1/items/{itemId}/categories",
        "Replacing categories must use PUT on the item route.");
    Assert(handler.LastBody?.Contains("\"category_ids\":[]") == true &&
        handler.LastBody.Contains("\"expected_revision\":\"6\"") && handler.LastToken == "secret",
        "Replacing categories must send the full selection and current revision with a session.");
}

static async Task InventoryPageCarriesCategoryFilter()
{
    var handler = new FakeHandler();
    var spaceId = Guid.NewGuid();
    var categoryId = Guid.NewGuid();
    var locationId = Guid.NewGuid();
    handler.Enqueue(HttpStatusCode.Created, """{"token":"secret","session":{"id":"session-1"}}""");
    handler.Enqueue(HttpStatusCode.OK, """{"items":[],"next_cursor":null}""");
    using var api = new SessionApi(handler);
    api.Configure("http://127.0.0.1:8080");
    await api.SignInAsync("root@example.org", "password");
    await api.GetItemsPageAsync(spaceId, after: "7", categoryId: categoryId, locationId: locationId);
    Assert(handler.LastQuery?.Contains($"category_id={categoryId}") == true &&
        handler.LastQuery.Contains($"location_id={locationId}") && handler.LastQuery.Contains("after=7"),
        "Category and location filters must be kept when loading another inventory page.");
}

static async Task ReadLocationsAndMoveItem()
{
    var handler = new FakeHandler();
    var spaceId = Guid.NewGuid();
    var rootId = Guid.NewGuid();
    var childId = Guid.NewGuid();
    var itemId = Guid.NewGuid();
    handler.Enqueue(HttpStatusCode.Created, """{"token":"secret","session":{"id":"session-1"}}""");
    handler.Enqueue(HttpStatusCode.OK,
        $"{{\"locations\":[{{\"id\":\"{rootId}\",\"space_id\":\"{spaceId}\",\"parent_id\":null,\"name\":\"Pièce\"}},{{\"id\":\"{childId}\",\"space_id\":\"{spaceId}\",\"parent_id\":\"{rootId}\",\"name\":\"Étagère\"}}]}}");
    handler.Enqueue(HttpStatusCode.OK,
        $"{{\"revision\":\"2\",\"location\":{{\"id\":\"{childId}\",\"space_id\":\"{spaceId}\",\"parent_id\":\"{rootId}\",\"name\":\"Étagère\"}}}}");
    handler.Enqueue(HttpStatusCode.OK, """{"revision":"3","location":null}""");
    handler.Enqueue(HttpStatusCode.OK,
        $"{{\"events\":[{{\"id\":\"{Guid.NewGuid()}\",\"from_location_id\":\"{childId}\",\"from_location_name\":\"Étagère\",\"to_location_id\":null,\"to_location_name\":null,\"actor_display_name\":\"Owner\",\"created_at\":\"2026-09-23T10:00:00Z\"}}]}}");
    using var api = new SessionApi(handler);
    api.Configure("http://127.0.0.1:8080");
    await api.SignInAsync("root@example.org", "password");
    var locations = await api.GetLocationsAsync(spaceId);
    Assert(locations.Count == 2 && locations[1].ParentId == rootId,
        "The location hierarchy must preserve parent IDs.");
    var current = await api.GetItemLocationAsync(itemId);
    Assert(current.Location?.Id == childId, "The sole current location must parse.");
    var cleared = await api.MoveItemLocationAsync(itemId, null, "2");
    Assert(cleared.Location is null && handler.LastMethod == HttpMethod.Put &&
        handler.LastBody?.Contains("\"location_id\":null") == true &&
        handler.LastBody.Contains("\"expected_revision\":\"2\""),
        "Removing a location must send an explicit null and current revision.");
    var events = await api.GetItemLocationEventsAsync(itemId);
    Assert(events.Single().Description.Contains("Sans emplacement") && handler.LastToken == "secret",
        "Movement history must be readable through the authenticated client.");
}

static async Task ReadStateAuditHistory()
{
    var handler = new FakeHandler();
    handler.Enqueue(HttpStatusCode.Created, """{"token":"secret","session":{"id":"session-1"}}""");
    handler.Enqueue(HttpStatusCode.OK,
        """{"events":[{"id":"01900000-0000-7000-8000-000000000003","actor_account_id":"01900000-0000-7000-8000-000000000004","actor_display_name":"Owner","state_before":"active","state_after":"archived","created_at":"2026-09-23T10:00:00Z"}]}""");
    using var api = new SessionApi(handler);
    api.Configure("http://127.0.0.1:8080");
    await api.SignInAsync("owner@example.org", "password");
    var itemId = Guid.Parse("01900000-0000-7000-8000-000000000001");
    var events = await api.GetItemStateEventsAsync(itemId);
    Assert(events.Count == 1 && events[0].Description == "Owner : Actif → Archivé",
        "Lifecycle audit events must parse with a readable label.");
    Assert(handler.LastPath == $"/api/v1/items/{itemId}/state-events",
        "Lifecycle history must target the selected item.");
    Assert(handler.LastToken == "secret", "Lifecycle history must carry the session token.");
}

static async Task ReadInventoryPage()
{
    var handler = new FakeHandler();
    handler.Enqueue(HttpStatusCode.Created, """{"token":"secret","session":{"id":"session-1"}}""");
    handler.Enqueue(HttpStatusCode.OK, """{"items":[],"next_cursor":"9"}""");
    using var api = new SessionApi(handler);
    api.Configure("http://127.0.0.1:8080");
    await api.SignInAsync("root@example.org", "password");
    var page = await api.GetItemsPageAsync(Guid.Parse("01900000-0000-7000-8000-000000000001"), "Casque 100%", "7", 2);
    Assert(page.NextCursor == "9", "The next page cursor must parse.");
    Assert(handler.LastQuery?.Contains("q=Casque%20100%25") == true, "Search text must be URL encoded.");
    Assert(handler.LastQuery?.Contains("after=7") == true && handler.LastQuery.Contains("limit=2"), "Pagination parameters must be sent.");
}

static async Task AcceptNewAccountInvitationWithoutLeakingTokenToUrl()
{
    var handler = new FakeHandler();
    handler.Enqueue(HttpStatusCode.NoContent, "");
    using var api = new SessionApi(handler);
    api.Configure("https://collectionops.example.org");
    var id = Guid.Parse("01900000-0000-7000-8000-000000000001");
    var token = new string('A', 43);
    await api.AcceptInvitationAsync($"collectionops://invite/{id}/{token}?server=https%3A%2F%2Fcollectionops.example.org%2F", "Invité", "test password");
    Assert(handler.LastPath == $"/api/v1/invitations/{id}/accept", "Invitation route must use the identifier only.");
    Assert(string.IsNullOrEmpty(handler.LastQuery), "The invitation token must not appear in the HTTP URL.");
    Assert(handler.LastToken is null, "New-account acceptance must not invent a session token.");
    Assert(handler.LastBody?.Contains($"\"token\":\"{token}\"") == true, "The invitation token must be sent in the request body.");
}

static async Task AcceptExistingAccountInvitationWithSession()
{
    var handler = new FakeHandler();
    handler.Enqueue(HttpStatusCode.Created, """{"token":"session-secret","session":{"id":"session-1"}}""");
    handler.Enqueue(HttpStatusCode.NoContent, "");
    using var api = new SessionApi(handler);
    api.Configure("https://collectionops.example.org");
    await api.SignInAsync("existing@example.org", "password");
    var id = Guid.Parse("01900000-0000-7000-8000-000000000001");
    await api.AcceptInvitationAsync($"collectionops://invite/{id}/{new string('A', 43)}?server=https%3A%2F%2Fcollectionops.example.org%2F", null, null);
    Assert(handler.LastToken == "session-secret", "Existing-account acceptance must send the session token.");
    Assert(handler.LastBody?.Contains("\"display_name\":null") == true, "Existing accounts must not try to sign up again.");
}

static async Task RejectInvitationForDifferentServer()
{
    using var api = new SessionApi(new FakeHandler());
    api.Configure("https://another.example.org");
    var id = Guid.Parse("01900000-0000-7000-8000-000000000001");
    await ExpectAsync<InvalidOperationException>(() => api.AcceptInvitationAsync(
        $"collectionops://invite/{id}/{new string('A', 43)}?server=https%3A%2F%2Fcollectionops.example.org%2F", null, null));
}

static async Task AdminCanListSpacesWithoutInventoryAccess()
{
    var handler = new FakeHandler();
    handler.Enqueue(HttpStatusCode.Created,
        """{"token":"secret","session":{"id":"session-1"},"principal":{"subject":"01900000-0000-7000-8000-000000000001","roles":["system_admin"]}}""");
    handler.Enqueue(HttpStatusCode.OK, """{"spaces":[]}""");
    using var api = new SessionApi(handler);
    api.Configure("http://127.0.0.1:8080");
    await api.SignInAsync("admin@example.org", "password");
    Assert(api.CurrentIsSystemAdmin, "The signed-in administrator role must be recognized.");
    await api.GetAdminSpacesAsync();
    Assert(handler.LastPath == "/api/v1/admin/spaces", "Administrators must use the separate administration route.");
    Assert(handler.LastToken == "secret", "Administration calls must send the session token.");
}

static async Task MemberGrantUpdateSendsExplicitRights()
{
    var handler = new FakeHandler();
    handler.Enqueue(HttpStatusCode.Created, """{"token":"secret","session":{"id":"session-1"}}""");
    handler.Enqueue(HttpStatusCode.OK,
        """{"account_id":"01900000-0000-7000-8000-000000000002","display_name":"Membre","email":"member@example.org","permissions":["collections_read","finance_read"]}""");
    using var api = new SessionApi(handler);
    api.Configure("http://127.0.0.1:8080");
    await api.SignInAsync("owner@example.org", "password");
    var space = Guid.Parse("01900000-0000-7000-8000-000000000001");
    var member = Guid.Parse("01900000-0000-7000-8000-000000000002");
    var result = await api.UpdateMemberPermissionsAsync(space, member, ["collections_read", "finance_read"]);
    Assert(result.Permissions.Contains("finance_read"), "The updated member grants must parse.");
    Assert(handler.LastPath == $"/api/v1/spaces/{space}/members/{member}/permissions", "Rights must target the selected member.");
    Assert(handler.LastBody?.Contains("\"finance_read\"") == true, "The financial grant must be explicit in the request.");
}

static async Task InventoryPageCarriesStateFilter()
{
    var handler = new FakeHandler();
    handler.Enqueue(HttpStatusCode.Created, """{"token":"secret","session":{"id":"session-1"}}""");
    handler.Enqueue(HttpStatusCode.OK, """{"items":[{"id":"01900000-0000-7000-8000-000000000001","space_id":"01900000-0000-7000-8000-000000000002","inventory_number":"3","name":"Objet","revision":"4","created_at":"2026-09-23T10:00:00Z","state":"trashed"}],"next_cursor":null}""");
    using var api = new SessionApi(handler);
    api.Configure("http://127.0.0.1:8080");
    await api.SignInAsync("root@example.org", "password");
    var page = await api.GetItemsPageAsync(Guid.Parse("01900000-0000-7000-8000-000000000002"), state: "trashed");
    Assert(page.Items.Count == 1 && page.Items[0].State == "trashed", "The item state must parse.");
    Assert(page.Items[0].StateLabel == "Corbeille", "The trashed state must have a French label.");
    Assert(handler.LastQuery?.Contains("state=trashed") == true, "The state filter must be sent.");
}

static async Task ArchiveUsesExpectedRevision()
{
    var handler = new FakeHandler();
    handler.Enqueue(HttpStatusCode.Created, """{"token":"secret","session":{"id":"session-1"}}""");
    handler.Enqueue(HttpStatusCode.OK, """{"id":"01900000-0000-7000-8000-000000000001","space_id":"01900000-0000-7000-8000-000000000002","inventory_number":"3","name":"Objet","revision":"2","created_at":"2026-09-23T10:00:00Z","state":"archived"}""");
    using var api = new SessionApi(handler);
    api.Configure("http://127.0.0.1:8080");
    await api.SignInAsync("root@example.org", "password");
    var itemId = Guid.Parse("01900000-0000-7000-8000-000000000001");
    var archived = await api.ArchiveItemAsync(itemId, "1");
    Assert(archived.State == "archived" && archived.Revision == "2", "The archived item must parse.");
    Assert(handler.LastPath == $"/api/v1/items/{itemId}/archive", "Archiving must target the selected item.");
    Assert(handler.LastBody?.Contains("\"expected_revision\":\"1\"") == true, "Archiving must send the displayed revision.");
    Assert(handler.LastToken == "secret", "Archiving must carry the session token.");
}

static async Task UpdateItemDetailsUsesRevision()
{
    var handler = new FakeHandler();
    handler.Enqueue(HttpStatusCode.Created, """{"token":"secret","session":{"id":"session-1"}}""");
    var itemId = Guid.NewGuid();
    handler.Enqueue(HttpStatusCode.OK,
        $"{{\"id\":\"{itemId}\",\"space_id\":\"{Guid.NewGuid()}\",\"inventory_number\":\"1\",\"name\":\"Casque\",\"description\":\"Ancien casque\",\"historical_reference\":\"Archive 42\",\"technical_reference\":\"Modèle B\",\"revision\":\"2\",\"created_at\":\"2026-09-23T10:00:00Z\"}}");
    using var api = new SessionApi(handler);
    api.Configure("http://127.0.0.1:8080");
    await api.SignInAsync("owner@example.org", "password");
    var updated = await api.UpdateItemDetailsAsync(itemId, "Ancien casque", "Archive 42", "Modèle B", "1");
    Assert(updated.Description == "Ancien casque" && updated.TechnicalReference == "Modèle B",
        "Item details must round-trip.");
    Assert(handler.LastPath == $"/api/v1/items/{itemId}/details" && handler.LastMethod == HttpMethod.Put,
        "Item details must use their dedicated route.");
    Assert(handler.LastBody?.Contains("\"expected_revision\":\"1\"") == true &&
        handler.LastBody.Contains("\"historical_reference\":\"Archive 42\"") &&
        handler.LastToken == "secret", "Details must carry the revision and token.");
}

static async Task AssignGroupsAndFilterInventory()
{
    var handler = new FakeHandler();
    handler.Enqueue(HttpStatusCode.Created, """{"token":"secret","session":{"id":"session-1"}}""");
    var itemId = Guid.NewGuid();
    var groupId = Guid.NewGuid();
    var spaceId = Guid.NewGuid();
    handler.Enqueue(HttpStatusCode.OK,
        $"{{\"revision\":\"3\",\"groups\":[{{\"id\":\"{groupId}\",\"space_id\":\"{spaceId}\",\"kind\":\"series\",\"name\":\"Casques 1944\"}}]}}");
    handler.Enqueue(HttpStatusCode.OK, """{"items":[],"next_cursor":null}""");
    using var api = new SessionApi(handler);
    api.Configure("http://127.0.0.1:8080");
    await api.SignInAsync("owner@example.org", "password");
    var assigned = await api.ReplaceItemGroupsAsync(itemId, [groupId], "2");
    Assert(assigned.Groups.Single().Label == "Série · Casques 1944", "Assigned series must parse.");
    Assert(handler.LastBody?.Contains($"\"{groupId}\"") == true &&
        handler.LastBody.Contains("\"expected_revision\":\"2\""),
        "Group assignment must send the selected ID and revision.");
    await api.GetItemsPageAsync(spaceId, groupId: groupId);
    Assert(handler.LastQuery?.Contains($"group_id={groupId}") == true,
        "Inventory filtering must include the group ID.");
}

static async Task RenameAndDeleteEmptyGroup()
{
    var handler = new FakeHandler();
    handler.Enqueue(HttpStatusCode.Created, """{"token":"secret","session":{"id":"session-1"}}""");
    var spaceId = Guid.NewGuid();
    var groupId = Guid.NewGuid();
    handler.Enqueue(HttpStatusCode.OK,
        $"{{\"id\":\"{groupId}\",\"space_id\":\"{spaceId}\",\"kind\":\"group\",\"name\":\"Ensemble A\",\"revision\":\"2\"}}");
    handler.Enqueue(HttpStatusCode.NoContent, "");
    using var api = new SessionApi(handler);
    api.Configure("http://127.0.0.1:8080");
    await api.SignInAsync("owner@example.org", "password");
    var renamed = await api.RenameGroupAsync(spaceId, groupId, "Ensemble A", "1");
    Assert(renamed.Revision == "2" && handler.LastMethod == HttpMethod.Patch &&
        handler.LastBody?.Contains("\"expected_revision\":\"1\"") == true,
        "Group rename must use optimistic revision checking.");
    await api.DeleteGroupAsync(spaceId, groupId, "2");
    Assert(handler.LastMethod == HttpMethod.Delete &&
        handler.LastPath == $"/api/v1/spaces/{spaceId}/groups/{groupId}" &&
        handler.LastBody?.Contains("\"expected_revision\":\"2\"") == true,
        "Group deletion must target the selected group and carry its revision.");
}

static async Task ReadAcquisitionPermissionsAndWishes()
{
    var handler = new FakeHandler();
    handler.Enqueue(HttpStatusCode.Created, """{"token":"secret","session":{"id":"session-1"}}""");
    handler.Enqueue(HttpStatusCode.OK, """{"permissions":["acquisitions_read"]}""");
    var spaceId = Guid.NewGuid();
    handler.Enqueue(HttpStatusCode.OK,
        $"{{\"wishes\":[{{\"id\":\"{Guid.NewGuid()}\",\"space_id\":\"{spaceId}\",\"title\":\"Casque M1\",\"search_notes\":\"Bon état\",\"revision\":\"1\",\"created_at\":\"2026-09-23T10:00:00Z\"}}]}}");
    using var api = new SessionApi(handler);
    api.Configure("http://127.0.0.1:8080");
    await api.SignInAsync("reader@example.org", "password");
    var rights = await api.GetMySpacePermissionsAsync(spaceId);
    Assert(rights.SequenceEqual(["acquisitions_read"]), "Acquisition rights must remain separate from collection and finance rights.");
    var wishes = await api.GetWishesAsync(spaceId);
    Assert(wishes.Count == 1 && wishes[0].Title == "Casque M1", "Wish list must parse.");
    Assert(handler.LastPath == $"/api/v1/spaces/{spaceId}/wishes" && handler.LastToken == "secret",
        "Wish list must target the space with the session token.");
}

static async Task CreateVendorAndOfferWithoutAmounts()
{
    var handler = new FakeHandler();
    handler.Enqueue(HttpStatusCode.Created, """{"token":"secret","session":{"id":"session-1"}}""");
    var spaceId = Guid.NewGuid();
    var wishId = Guid.NewGuid();
    var vendorId = Guid.NewGuid();
    handler.Enqueue(HttpStatusCode.Created,
        $"{{\"id\":\"{vendorId}\",\"space_id\":\"{spaceId}\",\"name\":\"Marchand A\",\"website_url\":null,\"revision\":\"1\",\"created_at\":\"2026-09-23T10:00:00Z\"}}");
    handler.Enqueue(HttpStatusCode.Created,
        $"{{\"id\":\"{Guid.NewGuid()}\",\"space_id\":\"{spaceId}\",\"wish_id\":\"{wishId}\",\"vendor_id\":\"{vendorId}\",\"title\":\"Casque original\",\"source_url\":null,\"notes\":null,\"revision\":\"1\",\"created_at\":\"2026-09-23T10:00:00Z\"}}");
    using var api = new SessionApi(handler);
    api.Configure("http://127.0.0.1:8080");
    await api.SignInAsync("writer@example.org", "password");
    var vendor = await api.CreateVendorAsync(spaceId, "Marchand A", null);
    Assert(vendor.Id == vendorId && vendor.Revision == "1" &&
        handler.LastBody?.Contains("\"name\":\"Marchand A\"") == true,
        "Vendor creation must target the chosen space.");
    var offer = await api.CreateOfferAsync(spaceId, wishId, vendorId, "Casque original", null, null);
    Assert(offer.VendorId == vendorId && offer.Revision == "1" &&
        handler.LastPath == $"/api/v1/spaces/{spaceId}/wishes/{wishId}/offers",
        "Offer creation must link the wish and vendor.");
    Assert(handler.LastBody?.Contains("\"vendor_id\"") == true &&
        !handler.LastBody.Contains("price") && !handler.LastBody.Contains("amount"),
        "This first acquisition slice must not send financial amounts.");
}

static async Task RenameCategoryAndFieldDefinitions()
{
    var handler = new FakeHandler();
    handler.Enqueue(HttpStatusCode.Created, """{"token":"secret","session":{"id":"session-1"}}""");
    var spaceId = Guid.NewGuid();
    var categoryId = Guid.NewGuid();
    var fieldId = Guid.NewGuid();
    handler.Enqueue(HttpStatusCode.OK,
        $"{{\"id\":\"{categoryId}\",\"space_id\":\"{spaceId}\",\"parent_id\":null,\"name\":\"Casques lourds\"}}");
    handler.Enqueue(HttpStatusCode.OK,
        $"{{\"id\":\"{fieldId}\",\"category_id\":\"{categoryId}\",\"name\":\"Materiau\",\"value_type\":\"text\"}}");
    using var api = new SessionApi(handler);
    api.Configure("http://127.0.0.1:8080");
    await api.SignInAsync("owner@example.org", "password");
    var category = await api.RenameCategoryAsync(spaceId, categoryId, "Casques lourds");
    Assert(category.Name == "Casques lourds" && handler.LastMethod == HttpMethod.Patch &&
        handler.LastPath == $"/api/v1/spaces/{spaceId}/categories/{categoryId}" &&
        handler.LastBody?.Contains("\"name\":\"Casques lourds\"") == true,
        "Category rename must PATCH the category with the new name.");
    var field = await api.RenameCategoryFieldAsync(spaceId, categoryId, fieldId, "Materiau");
    Assert(field.Name == "Materiau" && field.ValueType == "text" &&
        handler.LastPath == $"/api/v1/spaces/{spaceId}/categories/{categoryId}/fields/{fieldId}" &&
        handler.LastToken == "secret",
        "Field rename must keep the value type and target the field inside its category.");
}

static async Task ReadAndReplaceItemRelations()
{
    var handler = new FakeHandler();
    handler.Enqueue(HttpStatusCode.Created, """{"token":"secret","session":{"id":"session-1"}}""");
    var itemId = Guid.NewGuid();
    var variantId = Guid.NewGuid();
    handler.Enqueue(HttpStatusCode.OK,
        $"{{\"revision\":\"2\",\"relations\":[{{\"id\":\"{Guid.NewGuid()}\",\"kind\":\"variant_of\",\"direction\":\"outgoing\",\"related_item_id\":\"{variantId}\",\"related_item_name\":\"Casque M1\",\"created_at\":\"2026-09-24T10:00:00Z\"}}]}}");
    handler.Enqueue(HttpStatusCode.OK, """{"revision":"3","relations":[]}""");
    using var api = new SessionApi(handler);
    api.Configure("http://127.0.0.1:8080");
    await api.SignInAsync("owner@example.org", "password");
    var relations = await api.GetItemRelationsAsync(itemId);
    Assert(relations.Relations.Single().KindLabel == "Variante de" &&
        relations.Relations.Single().DirectionLabel == "Sortant" &&
        relations.Relations.Single().RelatedItemName == "Casque M1",
        "Relations must parse with readable labels.");
    Assert(handler.LastPath == $"/api/v1/items/{itemId}/relations" && handler.LastMethod == HttpMethod.Get,
        "Reading relations must target the item.");
    var replaced = await api.ReplaceItemRelationsAsync(itemId,
        [new ItemRelationInput(variantId, "variant_of")], "2");
    Assert(replaced.Revision == "3" && replaced.Relations.Count == 0 &&
        handler.LastMethod == HttpMethod.Put &&
        handler.LastBody?.Contains($"\"target_id\":\"{variantId}\"") == true &&
        handler.LastBody.Contains("\"kind\":\"variant_of\"") &&
        handler.LastBody.Contains("\"expected_revision\":\"2\"") && handler.LastToken == "secret",
        "Replacing relations must send the typed target and the displayed revision.");
}

static async Task TransferSpaceOwnership()
{
    var handler = new FakeHandler();
    handler.Enqueue(HttpStatusCode.Created, """{"token":"secret","session":{"id":"session-1"}}""");
    var spaceId = Guid.NewGuid();
    var previous = Guid.NewGuid();
    var newOwner = Guid.NewGuid();
    handler.Enqueue(HttpStatusCode.OK, $"{{\"space_id\":\"{spaceId}\",\"previous_owner_account_id\":\"{previous}\",\"new_owner_account_id\":\"{newOwner}\",\"actor_account_id\":\"{previous}\",\"transferred_at\":\"2026-09-24T10:00:00Z\"}}");
    using var api = new SessionApi(handler);
    api.Configure("http://127.0.0.1:8080");
    await api.SignInAsync("owner@example.org", "password");
    var result = await api.TransferSpaceOwnershipAsync(spaceId, newOwner);
    Assert(result.NewOwnerAccountId == newOwner && result.PreviousOwnerAccountId == previous,
        "Ownership transfer response must parse.");
    Assert(handler.LastPath == $"/api/v1/spaces/{spaceId}/ownership" && handler.LastMethod == HttpMethod.Post &&
        handler.LastBody?.Contains($"\"new_owner_account_id\":\"{newOwner}\"") == true &&
        handler.LastToken == "secret",
        "Ownership transfer must target the space and send the chosen member.");
}

static async Task UpdateAcquisitionProspectsWithRevision()
{
    var handler = new FakeHandler();
    handler.Enqueue(HttpStatusCode.Created, """{"token":"secret","session":{"id":"session-1"}}""");
    var spaceId = Guid.NewGuid();
    var wishId = Guid.NewGuid();
    var vendorId = Guid.NewGuid();
    var offerId = Guid.NewGuid();
    handler.Enqueue(HttpStatusCode.OK,
        $"{{\"id\":\"{wishId}\",\"space_id\":\"{spaceId}\",\"title\":\"Casque révisé\",\"revision\":\"2\",\"created_at\":\"2026-09-24T10:00:00Z\"}}");
    handler.Enqueue(HttpStatusCode.OK,
        $"{{\"id\":\"{vendorId}\",\"space_id\":\"{spaceId}\",\"name\":\"Marchand révisé\",\"revision\":\"3\",\"created_at\":\"2026-09-24T10:00:00Z\"}}");
    handler.Enqueue(HttpStatusCode.OK,
        $"{{\"id\":\"{offerId}\",\"space_id\":\"{spaceId}\",\"wish_id\":\"{wishId}\",\"vendor_id\":\"{vendorId}\",\"title\":\"Offre révisée\",\"revision\":\"4\",\"created_at\":\"2026-09-24T10:00:00Z\"}}");
    using var api = new SessionApi(handler);
    api.Configure("http://127.0.0.1:8080");
    await api.SignInAsync("writer@example.org", "password");
    var wish = await api.UpdateWishAsync(spaceId, wishId, "Casque révisé", null, "1");
    Assert(wish.Revision == "2" && handler.LastMethod == HttpMethod.Patch &&
        handler.LastPath == $"/api/v1/spaces/{spaceId}/wishes/{wishId}" &&
        handler.LastBody?.Contains("\"expected_revision\":\"1\"") == true,
        "Wish editing must carry the current revision.");
    var vendor = await api.UpdateVendorAsync(spaceId, vendorId, "Marchand révisé", null, "2");
    Assert(vendor.Revision == "3" && handler.LastPath == $"/api/v1/spaces/{spaceId}/vendors/{vendorId}" &&
        handler.LastBody?.Contains("\"expected_revision\":\"2\"") == true,
        "Vendor editing must target the chosen vendor and carry its revision.");
    var offer = await api.UpdateOfferAsync(spaceId, wishId, offerId, vendorId, "Offre révisée", null, null, "3");
    Assert(offer.Revision == "4" && handler.LastPath == $"/api/v1/spaces/{spaceId}/wishes/{wishId}/offers/{offerId}" &&
        handler.LastBody?.Contains("\"expected_revision\":\"3\"") == true &&
        !handler.LastBody.Contains("price") && !handler.LastBody.Contains("amount"),
        "Offer editing must keep the optimistic revision and remain non-financial.");
}

static void Assert(bool condition, string message)
{
    if (!condition) throw new Exception(message);
}

static void Expect<T>(Action action) where T : Exception
{
    try { action(); }
    catch (T) { return; }
    throw new Exception($"Expected {typeof(T).Name}.");
}

static async Task ExpectAsync<T>(Func<Task> action) where T : Exception
{
    try { await action(); }
    catch (T) { return; }
    throw new Exception($"Expected {typeof(T).Name}.");
}

sealed class FakeHandler : HttpMessageHandler
{
    private readonly Queue<HttpResponseMessage> _responses = new();
    public string? LastToken { get; private set; }
    public HttpMethod? LastMethod { get; private set; }
    public string? LastPath { get; private set; }
    public string? LastQuery { get; private set; }
    public string? LastBody { get; private set; }

    public void Enqueue(HttpStatusCode status, string body) => _responses.Enqueue(new HttpResponseMessage(status)
    {
        Content = new StringContent(body, Encoding.UTF8, "application/json"),
    });

    protected override async Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken)
    {
        LastMethod = request.Method;
        LastToken = request.Headers.TryGetValues("x-session-token", out var values)
            ? values.Single()
            : null;
        LastPath = request.RequestUri?.AbsolutePath;
        LastQuery = request.RequestUri?.Query;
        LastBody = request.Content is null ? null : await request.Content.ReadAsStringAsync(cancellationToken);
        return _responses.Dequeue();
    }
}

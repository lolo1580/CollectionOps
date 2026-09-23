using System.Net;
using System.Text;
using CollectionOps.Client.Services;

await RejectInsecureRemoteServer();
await RejectUnexpectedHealth();
await ClearExpiredSession();
await RejectMalformedJson();
await ReadCollectionWithSession();
await TransferUsesExpectedRevision();
await ReadTransferHistory();
await ReadInventoryPage();
await AcceptNewAccountInvitationWithoutLeakingTokenToUrl();
await AcceptExistingAccountInvitationWithSession();
await RejectInvitationForDifferentServer();
await AdminCanListSpacesWithoutInventoryAccess();
await MemberGrantUpdateSendsExplicitRights();
Console.WriteLine("SessionApi: 13 checks passed.");

static Task RejectInsecureRemoteServer()
{
    using var api = new SessionApi(new FakeHandler());
    Expect<ArgumentException>(() => api.Configure("http://example.org"));
    api.Configure("http://127.0.0.1:8080");
    return Task.CompletedTask;
}

static async Task RejectUnexpectedHealth()
{
    var handler = new FakeHandler();
    handler.Enqueue(HttpStatusCode.OK, """{"status":"unexpected"}""");
    using var api = new SessionApi(handler);
    api.Configure("http://127.0.0.1:8080");
    await ExpectAsync<InvalidOperationException>(() => api.CheckHealthAsync());
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
    public string? LastPath { get; private set; }
    public string? LastQuery { get; private set; }
    public string? LastBody { get; private set; }

    public void Enqueue(HttpStatusCode status, string body) => _responses.Enqueue(new HttpResponseMessage(status)
    {
        Content = new StringContent(body, Encoding.UTF8, "application/json"),
    });

    protected override async Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken)
    {
        LastToken = request.Headers.TryGetValues("x-session-token", out var values)
            ? values.Single()
            : null;
        LastPath = request.RequestUri?.AbsolutePath;
        LastQuery = request.RequestUri?.Query;
        LastBody = request.Content is null ? null : await request.Content.ReadAsStringAsync(cancellationToken);
        return _responses.Dequeue();
    }
}

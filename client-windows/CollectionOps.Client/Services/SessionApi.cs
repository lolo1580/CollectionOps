using System.Net;
using System.Net.Http.Json;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace CollectionOps.Client.Services;

// The session token is deliberately kept in memory until local encrypted storage is designed.
public sealed class SessionApi : IDisposable
{
    private readonly HttpClient _client;

    private Uri? _server;
    private string? _token;
    private bool _allowInsecurePrivateNetwork;

    public bool IsSignedIn => _token is not null;
    public string? ServerAddress => _server?.ToString();
    public bool AllowInsecurePrivateNetwork => _allowInsecurePrivateNetwork;
    public string? CurrentSessionId { get; private set; }
    public Guid? CurrentAccountId { get; private set; }
    public bool CurrentIsSystemAdmin { get; private set; }

    public SessionApi(HttpMessageHandler? handler = null)
    {
        _client = new HttpClient(handler ?? new HttpClientHandler { AllowAutoRedirect = false })
        {
            Timeout = TimeSpan.FromSeconds(15),
        };
    }

    public void Configure(string address) => Configure(address, _allowInsecurePrivateNetwork);

    public void Configure(string address, bool allowInsecurePrivateNetwork)
    {
        if (!Uri.TryCreate(address.Trim(), UriKind.Absolute, out var uri) ||
            !string.IsNullOrEmpty(uri.UserInfo) ||
            !string.IsNullOrEmpty(uri.Query) ||
            !string.IsNullOrEmpty(uri.Fragment) ||
            uri.AbsolutePath != "/" ||
            (uri.Scheme != Uri.UriSchemeHttps &&
             !(uri.Scheme == Uri.UriSchemeHttp &&
               (uri.IsLoopback || (allowInsecurePrivateNetwork && IsPrivateIpv4(uri))))))
        {
            throw new ArgumentException("Utilisez HTTPS, ou activez explicitement HTTP pour une adresse IP privée de développement.");
        }

        // Changing servers must never send a token issued by the previous server.
        if (_server != uri || _allowInsecurePrivateNetwork != allowInsecurePrivateNetwork)
        {
            SignOut();
        }

        _allowInsecurePrivateNetwork = allowInsecurePrivateNetwork;
        _server = uri;
    }

    private static bool IsPrivateIpv4(Uri uri)
    {
        if (uri.HostNameType != UriHostNameType.IPv4 || !IPAddress.TryParse(uri.Host, out var address))
        {
            return false;
        }

        var bytes = address.GetAddressBytes();
        return bytes[0] == 10 ||
               (bytes[0] == 172 && bytes[1] is >= 16 and <= 31) ||
               (bytes[0] == 192 && bytes[1] == 168);
    }

    public async Task<string> CheckHealthAsync()
    {
        using var response = await _client.GetAsync(Endpoint("api/v1/health/ready"));
        await EnsureSuccessAsync(response);
        var health = await response.Content.ReadFromJsonAsync<HealthResponse>();
        return health?.Status == "ready"
            ? "Serveur disponible"
            : throw new InvalidOperationException("Réponse de santé inattendue du serveur.");
    }

    public async Task SignInAsync(string email, string password)
    {
        if (string.IsNullOrWhiteSpace(email) || string.IsNullOrEmpty(password))
        {
            throw new ArgumentException("Saisissez votre adresse e-mail et votre mot de passe.");
        }

        using var response = await _client.PostAsJsonAsync(Endpoint("api/v1/sessions"), new
        {
            email,
            password,
            device_label = Environment.MachineName,
            idle_timeout = "minutes_30",
            stay_signed_in = false,
        });
        await EnsureSuccessAsync(response);
        var session = await response.Content.ReadFromJsonAsync<LoginResponse>();
        if (string.IsNullOrEmpty(session?.Token) || string.IsNullOrEmpty(session.Session?.Id))
        {
            throw new InvalidOperationException("La réponse de connexion est incomplète.");
        }

        _token = session.Token;
        CurrentSessionId = session.Session.Id;
        CurrentAccountId = session.Principal?.Subject;
        CurrentIsSystemAdmin = session.Principal?.Roles?.Contains("system_admin") == true;
    }

    public async Task<IReadOnlyList<SessionSummary>> GetSessionsAsync()
    {
        using var request = AuthenticatedRequest(HttpMethod.Get, "api/v1/sessions");
        using var response = await _client.SendAsync(request);
        if (response.StatusCode == HttpStatusCode.Unauthorized)
        {
            SignOut();
        }
        await EnsureSuccessAsync(response);
        var result = await response.Content.ReadFromJsonAsync<SessionListResponse>();
        return result?.Sessions
            ?? throw new InvalidOperationException("La liste des sessions est incomplète.");
    }

    public async Task<IReadOnlyList<CollectionSpace>> GetSpacesAsync()
    {
        var result = await SendJsonAsync<SpaceListResponse>(HttpMethod.Get, "api/v1/spaces");
        return result.Spaces;
    }

    public async Task<IReadOnlyList<CollectionSpace>> GetAdminSpacesAsync()
    {
        var result = await SendJsonAsync<SpaceListResponse>(HttpMethod.Get, "api/v1/admin/spaces");
        return result.Spaces;
    }

    public Task<CollectionSpace> CreateSpaceAsync(string name) =>
        SendJsonAsync<CollectionSpace>(HttpMethod.Post, "api/v1/spaces", new { name });

    public async Task<IReadOnlyList<string>> GetMySpacePermissionsAsync(Guid spaceId)
    {
        var result = await SendJsonAsync<SpacePermissionsResponse>(HttpMethod.Get,
            $"api/v1/spaces/{spaceId}/my-permissions");
        return result.Permissions;
    }

    public async Task<IReadOnlyList<WishEntry>> GetWishesAsync(Guid spaceId)
    {
        var result = await SendJsonAsync<WishListResponse>(HttpMethod.Get, $"api/v1/spaces/{spaceId}/wishes");
        return result.Wishes;
    }

    public Task<WishEntry> CreateWishAsync(Guid spaceId, string title, string? searchNotes) =>
        SendJsonAsync<WishEntry>(HttpMethod.Post, $"api/v1/spaces/{spaceId}/wishes",
            new { title, search_notes = searchNotes });

    public Task<WishEntry> UpdateWishAsync(Guid spaceId, Guid wishId, string title,
        string? searchNotes, string expectedRevision) =>
        SendJsonAsync<WishEntry>(HttpMethod.Patch, $"api/v1/spaces/{spaceId}/wishes/{wishId}",
            new { title, search_notes = searchNotes, expected_revision = expectedRevision });

    public async Task<IReadOnlyList<AcquisitionVendor>> GetVendorsAsync(Guid spaceId)
    {
        var result = await SendJsonAsync<VendorListResponse>(HttpMethod.Get,
            $"api/v1/spaces/{spaceId}/vendors");
        return result.Vendors;
    }

    public Task<AcquisitionVendor> CreateVendorAsync(Guid spaceId, string name, string? websiteUrl) =>
        SendJsonAsync<AcquisitionVendor>(HttpMethod.Post, $"api/v1/spaces/{spaceId}/vendors",
            new { name, website_url = websiteUrl });

    public Task<AcquisitionVendor> UpdateVendorAsync(Guid spaceId, Guid vendorId, string name,
        string? websiteUrl, string expectedRevision) =>
        SendJsonAsync<AcquisitionVendor>(HttpMethod.Patch, $"api/v1/spaces/{spaceId}/vendors/{vendorId}",
            new { name, website_url = websiteUrl, expected_revision = expectedRevision });

    public async Task<IReadOnlyList<AcquisitionOffer>> GetOffersAsync(Guid spaceId, Guid wishId)
    {
        var result = await SendJsonAsync<OfferListResponse>(HttpMethod.Get,
            $"api/v1/spaces/{spaceId}/wishes/{wishId}/offers");
        return result.Offers;
    }

    public Task<AcquisitionOffer> CreateOfferAsync(Guid spaceId, Guid wishId, Guid vendorId,
        string title, string? sourceUrl, string? notes) =>
        SendJsonAsync<AcquisitionOffer>(HttpMethod.Post,
            $"api/v1/spaces/{spaceId}/wishes/{wishId}/offers",
            new { vendor_id = vendorId, title, source_url = sourceUrl, notes });

    public Task<AcquisitionOffer> UpdateOfferAsync(Guid spaceId, Guid wishId, Guid offerId,
        Guid vendorId, string title, string? sourceUrl, string? notes, string expectedRevision) =>
        SendJsonAsync<AcquisitionOffer>(HttpMethod.Patch,
            $"api/v1/spaces/{spaceId}/wishes/{wishId}/offers/{offerId}",
            new { vendor_id = vendorId, title, source_url = sourceUrl, notes,
                expected_revision = expectedRevision });

    public async Task<IReadOnlyList<CollectionCategory>> GetCategoriesAsync(Guid spaceId)
    {
        var result = await SendJsonAsync<CategoryListResponse>(HttpMethod.Get,
            $"api/v1/spaces/{spaceId}/categories");
        return result.Categories;
    }

    public Task<CollectionCategory> CreateCategoryAsync(Guid spaceId, string name, Guid? parentId) =>
        SendJsonAsync<CollectionCategory>(HttpMethod.Post,
            $"api/v1/spaces/{spaceId}/categories", new { name, parent_id = parentId });

    public async Task<IReadOnlyList<CategoryFieldDefinition>> GetCategoryFieldsAsync(Guid spaceId, Guid categoryId)
    {
        var result = await SendJsonAsync<CategoryFieldListResponse>(HttpMethod.Get,
            $"api/v1/spaces/{spaceId}/categories/{categoryId}/fields");
        return result.Fields;
    }

    public Task<CategoryFieldDefinition> CreateCategoryFieldAsync(Guid spaceId, Guid categoryId, string name, string valueType) =>
        SendJsonAsync<CategoryFieldDefinition>(HttpMethod.Post,
            $"api/v1/spaces/{spaceId}/categories/{categoryId}/fields", new { name, value_type = valueType });

    public Task<CollectionCategory> RenameCategoryAsync(Guid spaceId, Guid categoryId, string name) =>
        SendJsonAsync<CollectionCategory>(HttpMethod.Patch,
            $"api/v1/spaces/{spaceId}/categories/{categoryId}", new { name });

    public Task<CategoryFieldDefinition> RenameCategoryFieldAsync(Guid spaceId, Guid categoryId, Guid fieldId, string name) =>
        SendJsonAsync<CategoryFieldDefinition>(HttpMethod.Patch,
            $"api/v1/spaces/{spaceId}/categories/{categoryId}/fields/{fieldId}", new { name });

    public async Task<IReadOnlyList<CollectionLocation>> GetLocationsAsync(Guid spaceId)
    {
        var result = await SendJsonAsync<LocationListResponse>(HttpMethod.Get,
            $"api/v1/spaces/{spaceId}/locations");
        return result.Locations;
    }

    public Task<CollectionLocation> CreateLocationAsync(Guid spaceId, string name, Guid? parentId) =>
        SendJsonAsync<CollectionLocation>(HttpMethod.Post,
            $"api/v1/spaces/{spaceId}/locations", new { name, parent_id = parentId });

    public async Task<IReadOnlyList<CollectionGroup>> GetGroupsAsync(Guid spaceId)
    {
        var result = await SendJsonAsync<GroupListResponse>(HttpMethod.Get,
            $"api/v1/spaces/{spaceId}/groups");
        return result.Groups;
    }

    public Task<CollectionGroup> CreateGroupAsync(Guid spaceId, string kind, string name) =>
        SendJsonAsync<CollectionGroup>(HttpMethod.Post,
            $"api/v1/spaces/{spaceId}/groups", new { kind, name });

    public Task<CollectionGroup> RenameGroupAsync(Guid spaceId, Guid groupId, string name, string expectedRevision) =>
        SendJsonAsync<CollectionGroup>(HttpMethod.Patch,
            $"api/v1/spaces/{spaceId}/groups/{groupId}", new { name, expected_revision = expectedRevision });

    public async Task DeleteGroupAsync(Guid spaceId, Guid groupId, string expectedRevision)
    {
        using var request = AuthenticatedRequest(HttpMethod.Delete,
            $"api/v1/spaces/{spaceId}/groups/{groupId}");
        request.Content = JsonContent.Create(new { expected_revision = expectedRevision });
        using var response = await _client.SendAsync(request);
        if (response.StatusCode == HttpStatusCode.Unauthorized) SignOut();
        await EnsureSuccessAsync(response);
    }

    public Task<ItemGroupListResponse> GetItemGroupsAsync(Guid itemId) =>
        SendJsonAsync<ItemGroupListResponse>(HttpMethod.Get, $"api/v1/items/{itemId}/groups");

    public Task<ItemGroupListResponse> ReplaceItemGroupsAsync(Guid itemId, IReadOnlyList<Guid> groupIds, string expectedRevision) =>
        SendJsonAsync<ItemGroupListResponse>(HttpMethod.Put, $"api/v1/items/{itemId}/groups",
            new { group_ids = groupIds, expected_revision = expectedRevision });

    public Task<ItemRelationListResponse> GetItemRelationsAsync(Guid itemId) =>
        SendJsonAsync<ItemRelationListResponse>(HttpMethod.Get, $"api/v1/items/{itemId}/relations");

    public Task<ItemRelationListResponse> ReplaceItemRelationsAsync(Guid itemId,
        IReadOnlyList<ItemRelationInput> relations, string expectedRevision) =>
        SendJsonAsync<ItemRelationListResponse>(HttpMethod.Put, $"api/v1/items/{itemId}/relations",
            new { relations, expected_revision = expectedRevision });

    public async Task<IReadOnlyList<SpaceInvitation>> GetInvitationsAsync(Guid spaceId)
    {
        var result = await SendJsonAsync<InvitationListResponse>(HttpMethod.Get, $"api/v1/spaces/{spaceId}/invitations");
        return result.Invitations;
    }

    public async Task<IReadOnlyList<SpaceMember>> GetMembersAsync(Guid spaceId)
    {
        var result = await SendJsonAsync<SpaceMemberListResponse>(HttpMethod.Get, $"api/v1/spaces/{spaceId}/members");
        return result.Members;
    }

    public Task<SpaceMember> UpdateMemberPermissionsAsync(Guid spaceId, Guid accountId, IReadOnlyList<string> permissions) =>
        SendJsonAsync<SpaceMember>(HttpMethod.Put, $"api/v1/spaces/{spaceId}/members/{accountId}/permissions", new { permissions });

    public Task<SpaceOwnershipTransfer> TransferSpaceOwnershipAsync(Guid spaceId, Guid newOwnerAccountId) =>
        SendJsonAsync<SpaceOwnershipTransfer>(HttpMethod.Post, $"api/v1/spaces/{spaceId}/ownership",
            new { new_owner_account_id = newOwnerAccountId });

    public async Task<IReadOnlyList<SpaceManager>> GetSpaceManagersAsync(Guid spaceId)
    {
        var result = await SendJsonAsync<SpaceManagerListResponse>(HttpMethod.Get,
            $"api/v1/spaces/{spaceId}/managers");
        return result.Managers;
    }

    public Task AppointSpaceManagerAsync(Guid spaceId, Guid accountId) =>
        SendEmptyAsync(HttpMethod.Put, $"api/v1/spaces/{spaceId}/managers/{accountId}");

    public Task RevokeSpaceManagerAsync(Guid spaceId, Guid accountId) =>
        SendEmptyAsync(HttpMethod.Delete, $"api/v1/spaces/{spaceId}/managers/{accountId}");

    public async Task RemoveMemberAsync(Guid spaceId, Guid accountId)
    {
        using var request = AuthenticatedRequest(HttpMethod.Delete, $"api/v1/spaces/{spaceId}/members/{accountId}");
        using var response = await _client.SendAsync(request);
        await EnsureSuccessAsync(response);
    }

    public Task<SpaceInvitation> InviteAsync(Guid spaceId, string email, IReadOnlyList<string> permissions) =>
        SendJsonAsync<SpaceInvitation>(HttpMethod.Post, $"api/v1/spaces/{spaceId}/invitations", new { email, permissions });

    public async Task RevokeInvitationAsync(Guid spaceId, Guid invitationId)
    {
        using var request = AuthenticatedRequest(HttpMethod.Delete, $"api/v1/spaces/{spaceId}/invitations/{invitationId}");
        using var response = await _client.SendAsync(request);
        await EnsureSuccessAsync(response);
    }

    public async Task AcceptInvitationAsync(string link, string? name, string? password)
    {
        if (!Uri.TryCreate(link.Trim(), UriKind.Absolute, out var uri) ||
            uri.Scheme != "collectionops" || uri.Host != "invite" ||
            !uri.Query.StartsWith("?server=", StringComparison.Ordinal) ||
            !string.IsNullOrEmpty(uri.Fragment))
            throw new ArgumentException("Collez un lien d'invitation CollectionOps valide.");
        var server = Uri.UnescapeDataString(uri.Query["?server=".Length..]);
        if (!Uri.TryCreate(server, UriKind.Absolute, out var expectedServer) ||
            expectedServer.Scheme != Uri.UriSchemeHttps ||
            !string.IsNullOrEmpty(expectedServer.UserInfo) ||
            !string.IsNullOrEmpty(expectedServer.Query) ||
            !string.IsNullOrEmpty(expectedServer.Fragment) || expectedServer.AbsolutePath != "/")
            throw new ArgumentException("Le serveur indiqué dans l'invitation est invalide.");
        if (_server != expectedServer)
            throw new InvalidOperationException($"Réglez d'abord l'adresse du serveur sur {expectedServer}.");
        var parts = uri.AbsolutePath.Trim('/').Split('/');
        if (parts.Length != 2 || !Guid.TryParse(parts[0], out var id) || parts[1].Length != 43)
            throw new ArgumentException("Collez un lien d'invitation CollectionOps valide.");
        using var request = new HttpRequestMessage(HttpMethod.Post, Endpoint($"api/v1/invitations/{id}/accept"));
        if (_token is not null) request.Headers.Add("x-session-token", _token);
        request.Content = JsonContent.Create(new { token = parts[1], display_name = name, password });
        using var response = await _client.SendAsync(request);
        await EnsureSuccessAsync(response);
    }

    public Task<ItemPageResponse> GetItemsPageAsync(Guid spaceId, string? search = null, string? after = null, int limit = 50, string? state = null, Guid? categoryId = null, Guid? locationId = null, Guid? groupId = null)
    {
        if (limit is < 1 or > 100) throw new ArgumentOutOfRangeException(nameof(limit));
        var parameters = new List<string> { $"limit={limit}" };
        if (!string.IsNullOrWhiteSpace(search)) parameters.Add($"q={Uri.EscapeDataString(search.Trim())}");
        if (!string.IsNullOrEmpty(after)) parameters.Add($"after={Uri.EscapeDataString(after)}");
        if (!string.IsNullOrWhiteSpace(state)) parameters.Add($"state={Uri.EscapeDataString(state)}");
        if (categoryId is { } id) parameters.Add($"category_id={id}");
        if (locationId is { } location) parameters.Add($"location_id={location}");
        if (groupId is { } group) parameters.Add($"group_id={group}");
        return SendJsonAsync<ItemPageResponse>(HttpMethod.Get,
            $"api/v1/spaces/{spaceId}/items?{string.Join("&", parameters)}");
    }

    public Task<InventoryItem> CreateItemAsync(Guid spaceId, string name) =>
        SendJsonAsync<InventoryItem>(HttpMethod.Post, $"api/v1/spaces/{spaceId}/items", new { name });

    public Task<InventoryItem> GetItemAsync(Guid itemId) =>
        SendJsonAsync<InventoryItem>(HttpMethod.Get, $"api/v1/items/{itemId}");

    public Task<InventoryItem> RenameItemAsync(Guid itemId, string name, string expectedRevision) =>
        SendJsonAsync<InventoryItem>(HttpMethod.Patch, $"api/v1/items/{itemId}",
            new { name, expected_revision = expectedRevision });

    public Task<InventoryItem> UpdateItemDetailsAsync(Guid itemId, string? description,
        string? historicalReference, string? technicalReference, string expectedRevision) =>
        SendJsonAsync<InventoryItem>(HttpMethod.Put, $"api/v1/items/{itemId}/details",
            new { description, historical_reference = historicalReference,
                technical_reference = technicalReference, expected_revision = expectedRevision });

    public Task<InventoryItem> ArchiveItemAsync(Guid itemId, string expectedRevision) =>
        SendJsonAsync<InventoryItem>(HttpMethod.Post, $"api/v1/items/{itemId}/archive",
            new { expected_revision = expectedRevision });

    public Task<InventoryItem> TrashItemAsync(Guid itemId, string expectedRevision) =>
        SendJsonAsync<InventoryItem>(HttpMethod.Post, $"api/v1/items/{itemId}/trash",
            new { expected_revision = expectedRevision });

    public Task<InventoryItem> RestoreItemAsync(Guid itemId, string expectedRevision) =>
        SendJsonAsync<InventoryItem>(HttpMethod.Post, $"api/v1/items/{itemId}/restore",
            new { expected_revision = expectedRevision });

    public Task<ItemCategoryListResponse> GetItemCategoriesAsync(Guid itemId) =>
        SendJsonAsync<ItemCategoryListResponse>(HttpMethod.Get, $"api/v1/items/{itemId}/categories");

    public Task<ItemCategoryListResponse> AddItemCategoriesAsync(Guid itemId, IReadOnlyList<Guid> categoryIds, string expectedRevision) =>
        SendJsonAsync<ItemCategoryListResponse>(HttpMethod.Post, $"api/v1/items/{itemId}/categories",
            new { category_ids = categoryIds, expected_revision = expectedRevision });

    public Task<ItemCategoryListResponse> ReplaceItemCategoriesAsync(Guid itemId, IReadOnlyList<Guid> categoryIds, string expectedRevision) =>
        SendJsonAsync<ItemCategoryListResponse>(HttpMethod.Put, $"api/v1/items/{itemId}/categories",
            new { category_ids = categoryIds, expected_revision = expectedRevision });

    public Task<EffectiveFieldListResponse> GetItemFieldsAsync(Guid itemId) =>
        SendJsonAsync<EffectiveFieldListResponse>(HttpMethod.Get, $"api/v1/items/{itemId}/fields");

    public Task<EffectiveFieldListResponse> SetItemFieldValueAsync(Guid itemId, Guid fieldId, string value, string expectedRevision) =>
        SendJsonAsync<EffectiveFieldListResponse>(HttpMethod.Put, $"api/v1/items/{itemId}/fields/{fieldId}",
            new { value, expected_revision = expectedRevision });

    public Task<ItemLocationResponse> GetItemLocationAsync(Guid itemId) =>
        SendJsonAsync<ItemLocationResponse>(HttpMethod.Get, $"api/v1/items/{itemId}/location");

    public Task<ItemLocationResponse> MoveItemLocationAsync(Guid itemId, Guid? locationId, string expectedRevision) =>
        SendJsonAsync<ItemLocationResponse>(HttpMethod.Put, $"api/v1/items/{itemId}/location",
            new { location_id = locationId, expected_revision = expectedRevision });

    public async Task<IReadOnlyList<ItemLocationEvent>> GetItemLocationEventsAsync(Guid itemId)
    {
        var result = await SendJsonAsync<ItemLocationEventListResponse>(HttpMethod.Get,
            $"api/v1/items/{itemId}/location-events");
        return result.Events;
    }

    public Task<TransferOutcome> TransferItemAsync(Guid itemId, Guid destinationSpaceId, string expectedRevision,
        IReadOnlyList<Guid>? destinationCategoryIds = null) =>
        SendJsonAsync<TransferOutcome>(HttpMethod.Post, $"api/v1/items/{itemId}/transfers",
            new { destination_space_id = destinationSpaceId, expected_revision = expectedRevision,
                destination_category_ids = destinationCategoryIds });

    public async Task<IReadOnlyList<InventoryTransfer>> GetItemTransfersAsync(Guid itemId)
    {
        var result = await SendJsonAsync<TransferListResponse>(HttpMethod.Get, $"api/v1/items/{itemId}/transfers");
        return result.Transfers;
    }

    public async Task<IReadOnlyList<ItemStateAuditEvent>> GetItemStateEventsAsync(Guid itemId)
    {
        var result = await SendJsonAsync<ItemStateEventListResponse>(HttpMethod.Get,
            $"api/v1/items/{itemId}/state-events");
        return result.Events;
    }

    public async Task RevokeAsync(string sessionId)
    {
        using var request = AuthenticatedRequest(HttpMethod.Delete, $"api/v1/sessions/{Uri.EscapeDataString(sessionId)}");
        using var response = await _client.SendAsync(request);
        if (response.StatusCode == HttpStatusCode.Unauthorized)
        {
            SignOut();
        }
        await EnsureSuccessAsync(response);
        if (sessionId == CurrentSessionId)
        {
            SignOut();
        }
    }

    public void SignOut()
    {
        _token = null;
        CurrentSessionId = null;
        CurrentAccountId = null;
        CurrentIsSystemAdmin = false;
    }

    public void Dispose() => _client.Dispose();

    private Uri Endpoint(string path) => _server is null
        ? throw new InvalidOperationException("Indiquez d'abord l'adresse du serveur.")
        : new Uri(_server, path);

    private HttpRequestMessage AuthenticatedRequest(HttpMethod method, string path)
    {
        if (_token is null)
        {
            throw new InvalidOperationException("Connectez-vous d'abord.");
        }

        var request = new HttpRequestMessage(method, Endpoint(path));
        request.Headers.Add("x-session-token", _token);
        return request;
    }

    private async Task<T> SendJsonAsync<T>(HttpMethod method, string path, object? body = null)
    {
        using var request = AuthenticatedRequest(method, path);
        if (body is not null)
        {
            request.Content = JsonContent.Create(body);
        }
        using var response = await _client.SendAsync(request);
        if (response.StatusCode == HttpStatusCode.Unauthorized)
        {
            SignOut();
        }
        await EnsureSuccessAsync(response);
        return await response.Content.ReadFromJsonAsync<T>()
            ?? throw new InvalidOperationException("La réponse du serveur est incomplète.");
    }

    private async Task SendEmptyAsync(HttpMethod method, string path)
    {
        using var request = AuthenticatedRequest(method, path);
        using var response = await _client.SendAsync(request);
        if (response.StatusCode == HttpStatusCode.Unauthorized)
        {
            SignOut();
        }
        await EnsureSuccessAsync(response);
    }

    private static async Task EnsureSuccessAsync(HttpResponseMessage response)
    {
        if (response.IsSuccessStatusCode)
        {
            return;
        }

        if (response.StatusCode == HttpStatusCode.Unauthorized)
        {
            throw new InvalidOperationException("Identifiants refusés ou session expirée.");
        }

        if (response.StatusCode == HttpStatusCode.ServiceUnavailable)
        {
            try
            {
                var problem = await response.Content.ReadFromJsonAsync<ProblemResponse>();
                if (!string.IsNullOrWhiteSpace(problem?.Detail))
                    throw new InvalidOperationException(problem.Detail);
            }
            catch (JsonException) { }
            throw new InvalidOperationException("Le service est temporairement indisponible.");
        }

        try
        {
            var problem = await response.Content.ReadFromJsonAsync<ProblemResponse>();
            if (!string.IsNullOrWhiteSpace(problem?.Detail))
            {
                throw new InvalidOperationException(problem.Detail);
            }
        }
        catch (JsonException)
        {
            // A proxy can return HTML instead of the API's problem JSON.
        }

        if (response.StatusCode == HttpStatusCode.NotFound)
        {
            throw new InvalidOperationException("Cette ressource est introuvable ou inaccessible.");
        }

        throw new InvalidOperationException($"Le serveur a répondu {(int)response.StatusCode}.");
    }
}

public sealed record SessionSummary(
    [property: JsonPropertyName("id")] string Id,
    [property: JsonPropertyName("device_label")] string? DeviceLabel,
    [property: JsonPropertyName("last_seen_at")] DateTimeOffset LastSeenAt,
    [property: JsonPropertyName("revoked_at")] DateTimeOffset? RevokedAt);

public sealed record SessionListResponse([property: JsonPropertyName("sessions")] List<SessionSummary> Sessions);
public sealed record LoginResponse(
    [property: JsonPropertyName("token")] string Token,
    [property: JsonPropertyName("session")] SessionSummary Session,
    [property: JsonPropertyName("principal")] SessionPrincipal? Principal);
public sealed record SessionPrincipal(
    [property: JsonPropertyName("subject")] Guid Subject,
    [property: JsonPropertyName("roles")] List<string>? Roles);
public sealed record HealthResponse([property: JsonPropertyName("status")] string Status);
public sealed record ProblemResponse([property: JsonPropertyName("detail")] string? Detail);
public sealed record CollectionSpace(
    [property: JsonPropertyName("id")] Guid Id,
    [property: JsonPropertyName("name")] string Name,
    [property: JsonPropertyName("owner_account_id")] Guid OwnerAccountId);
public sealed record SpaceListResponse([property: JsonPropertyName("spaces")] List<CollectionSpace> Spaces);
public sealed record SpacePermissionsResponse([property: JsonPropertyName("permissions")] List<string> Permissions);
public sealed record WishEntry(
    [property: JsonPropertyName("id")] Guid Id,
    [property: JsonPropertyName("space_id")] Guid SpaceId,
    [property: JsonPropertyName("title")] string Title,
    [property: JsonPropertyName("search_notes")] string? SearchNotes,
    [property: JsonPropertyName("revision")] string Revision,
    [property: JsonPropertyName("created_at")] DateTimeOffset CreatedAt);
public sealed record WishListResponse([property: JsonPropertyName("wishes")] List<WishEntry> Wishes);
public sealed record AcquisitionVendor(
    [property: JsonPropertyName("id")] Guid Id,
    [property: JsonPropertyName("space_id")] Guid SpaceId,
    [property: JsonPropertyName("name")] string Name,
    [property: JsonPropertyName("website_url")] string? WebsiteUrl,
    [property: JsonPropertyName("revision")] string Revision,
    [property: JsonPropertyName("created_at")] DateTimeOffset CreatedAt)
{
    public string Label => WebsiteUrl is null ? Name : $"{Name} · {WebsiteUrl}";
}
public sealed record VendorListResponse([property: JsonPropertyName("vendors")] List<AcquisitionVendor> Vendors);
public sealed record AcquisitionOffer(
    [property: JsonPropertyName("id")] Guid Id,
    [property: JsonPropertyName("space_id")] Guid SpaceId,
    [property: JsonPropertyName("wish_id")] Guid WishId,
    [property: JsonPropertyName("vendor_id")] Guid VendorId,
    [property: JsonPropertyName("title")] string Title,
    [property: JsonPropertyName("source_url")] string? SourceUrl,
    [property: JsonPropertyName("notes")] string? Notes,
    [property: JsonPropertyName("revision")] string Revision,
    [property: JsonPropertyName("created_at")] DateTimeOffset CreatedAt);
public sealed record OfferListResponse([property: JsonPropertyName("offers")] List<AcquisitionOffer> Offers);
public sealed record OfferLine(AcquisitionOffer Offer, string Label);
public sealed record SpaceInvitation(
    [property: JsonPropertyName("id")] Guid Id,
    [property: JsonPropertyName("recipient_email")] string RecipientEmail,
    [property: JsonPropertyName("expires_at")] DateTimeOffset ExpiresAt,
    [property: JsonPropertyName("accepted_at")] DateTimeOffset? AcceptedAt,
    [property: JsonPropertyName("revoked_at")] DateTimeOffset? RevokedAt,
    [property: JsonPropertyName("permissions")] List<string> Permissions)
{
    public string Status => AcceptedAt is not null ? "Acceptée" : RevokedAt is not null ? "Révoquée" : ExpiresAt <= DateTimeOffset.UtcNow ? "Expirée" : "En attente";
    public bool IsPending => AcceptedAt is null && RevokedAt is null && ExpiresAt > DateTimeOffset.UtcNow;
    public string PermissionsSummary => string.Join(", ", Permissions.Select(permission => permission switch
    {
        "collections_read" => "collection : lecture",
        "collections_write" => "collection : écriture",
        "acquisitions_read" => "acquisitions : lecture",
        "acquisitions_write" => "acquisitions : écriture",
        "finance_read" => "finances : lecture",
        "finance_write" => "finances : écriture",
        "documents_read" => "documents : lecture",
        "documents_write" => "documents : écriture",
        _ => permission,
    }));
}
public sealed record InvitationListResponse([property: JsonPropertyName("invitations")] List<SpaceInvitation> Invitations);
public sealed record SpaceMember(
    [property: JsonPropertyName("account_id")] Guid AccountId,
    [property: JsonPropertyName("display_name")] string DisplayName,
    [property: JsonPropertyName("email")] string? Email,
    [property: JsonPropertyName("permissions")] List<string> Permissions)
{
    public string Label => Email is null ? DisplayName : $"{DisplayName} ({Email})";
}
public sealed record SpaceMemberListResponse([property: JsonPropertyName("members")] List<SpaceMember> Members);
public sealed record SpaceOwnershipTransfer(
    [property: JsonPropertyName("space_id")] Guid SpaceId,
    [property: JsonPropertyName("previous_owner_account_id")] Guid PreviousOwnerAccountId,
    [property: JsonPropertyName("new_owner_account_id")] Guid NewOwnerAccountId,
    [property: JsonPropertyName("actor_account_id")] Guid ActorAccountId,
    [property: JsonPropertyName("transferred_at")] DateTimeOffset TransferredAt);
public sealed record SpaceManager(
    [property: JsonPropertyName("account_id")] Guid AccountId,
    [property: JsonPropertyName("display_name")] string DisplayName);
public sealed record SpaceManagerListResponse(
    [property: JsonPropertyName("managers")] List<SpaceManager> Managers);
public sealed record CollectionCategory(
    [property: JsonPropertyName("id")] Guid Id,
    [property: JsonPropertyName("space_id")] Guid SpaceId,
    [property: JsonPropertyName("parent_id")] Guid? ParentId,
    [property: JsonPropertyName("name")] string Name);
public sealed record CategoryListResponse(
    [property: JsonPropertyName("categories")] List<CollectionCategory> Categories);
public sealed record CategoryOption(Guid? Id, string Label);
public sealed record CategoryFieldDefinition(
    [property: JsonPropertyName("id")] Guid Id,
    [property: JsonPropertyName("category_id")] Guid CategoryId,
    [property: JsonPropertyName("name")] string Name,
    [property: JsonPropertyName("value_type")] string ValueType)
{
    public string Label => $"{Name} ({ValueType})";
}
public sealed record CategoryFieldListResponse(
    [property: JsonPropertyName("fields")] List<CategoryFieldDefinition> Fields);
public sealed record CollectionLocation(
    [property: JsonPropertyName("id")] Guid Id,
    [property: JsonPropertyName("space_id")] Guid SpaceId,
    [property: JsonPropertyName("parent_id")] Guid? ParentId,
    [property: JsonPropertyName("name")] string Name);
public sealed record CollectionGroup(
    [property: JsonPropertyName("id")] Guid Id,
    [property: JsonPropertyName("space_id")] Guid SpaceId,
    [property: JsonPropertyName("kind")] string Kind,
    [property: JsonPropertyName("name")] string Name,
    [property: JsonPropertyName("revision")] string Revision)
{
    public string Label => $"{(Kind == "series" ? "Série" : "Regroupement")} · {Name}";
}
public sealed record GroupListResponse([property: JsonPropertyName("groups")] List<CollectionGroup> Groups);
public sealed record ItemGroupListResponse(
    [property: JsonPropertyName("revision")] string Revision,
    [property: JsonPropertyName("groups")] List<CollectionGroup> Groups);
public sealed record GroupOption(Guid? Id, string Label);
public sealed record ItemRelation(
    [property: JsonPropertyName("id")] Guid Id,
    [property: JsonPropertyName("kind")] string Kind,
    [property: JsonPropertyName("direction")] string Direction,
    [property: JsonPropertyName("related_item_id")] Guid RelatedItemId,
    [property: JsonPropertyName("related_item_name")] string RelatedItemName,
    [property: JsonPropertyName("created_at")] DateTimeOffset CreatedAt)
{
    public string KindLabel => Kind switch
    {
        "variant_of" => "Variante de",
        "part_of" => "Fait partie de",
        _ => "Lié",
    };
    public string DirectionLabel => Direction == "outgoing" ? "Sortant" : "Entrant";
}
public sealed record ItemRelationListResponse(
    [property: JsonPropertyName("revision")] string Revision,
    [property: JsonPropertyName("relations")] List<ItemRelation> Relations);
public sealed record ItemRelationInput(
    [property: JsonPropertyName("target_id")] Guid TargetId,
    [property: JsonPropertyName("kind")] string Kind);
public sealed record RelationOption(Guid Id, string Label);
public sealed record LocationListResponse(
    [property: JsonPropertyName("locations")] List<CollectionLocation> Locations);
public sealed record LocationOption(Guid? Id, string Label);
public sealed record ItemLocationResponse(
    [property: JsonPropertyName("revision")] string Revision,
    [property: JsonPropertyName("location")] CollectionLocation? Location);
public sealed record ItemLocationEvent(
    [property: JsonPropertyName("id")] Guid Id,
    [property: JsonPropertyName("from_location_id")] Guid? FromLocationId,
    [property: JsonPropertyName("from_location_name")] string? FromLocationName,
    [property: JsonPropertyName("to_location_id")] Guid? ToLocationId,
    [property: JsonPropertyName("to_location_name")] string? ToLocationName,
    [property: JsonPropertyName("actor_display_name")] string ActorDisplayName,
    [property: JsonPropertyName("created_at")] DateTimeOffset CreatedAt)
{
    public string Description => $"{ActorDisplayName} : {FromLocationName ?? "Sans emplacement"} → {ToLocationName ?? "Sans emplacement"}";
    public string Date => CreatedAt.ToLocalTime().ToString("g");
}
public sealed record ItemLocationEventListResponse(
    [property: JsonPropertyName("events")] List<ItemLocationEvent> Events);
public sealed record ItemCategoryAssignment(
    [property: JsonPropertyName("category_id")] Guid CategoryId,
    [property: JsonPropertyName("category_name")] string CategoryName);
public sealed record ItemCategoryListResponse(
    [property: JsonPropertyName("revision")] string Revision,
    [property: JsonPropertyName("categories")] List<ItemCategoryAssignment> Categories);
public sealed record EffectiveItemField(
    [property: JsonPropertyName("id")] Guid Id,
    [property: JsonPropertyName("defined_category_name")] string DefinedCategoryName,
    [property: JsonPropertyName("name")] string Name,
    [property: JsonPropertyName("value_type")] string ValueType,
    [property: JsonPropertyName("value")] string? Value)
{
    public string Label => $"{DefinedCategoryName} · {Name} ({ValueType}) : {Value ?? "—"}";
}
public sealed record EffectiveFieldListResponse(
    [property: JsonPropertyName("revision")] string Revision,
    [property: JsonPropertyName("fields")] List<EffectiveItemField> Fields);
public sealed record InventoryItem(
    [property: JsonPropertyName("id")] Guid Id,
    [property: JsonPropertyName("space_id")] Guid SpaceId,
    [property: JsonPropertyName("inventory_number")] string InventoryNumber,
    [property: JsonPropertyName("name")] string Name,
    [property: JsonPropertyName("description")] string? Description,
    [property: JsonPropertyName("historical_reference")] string? HistoricalReference,
    [property: JsonPropertyName("technical_reference")] string? TechnicalReference,
    [property: JsonPropertyName("revision")] string Revision,
    [property: JsonPropertyName("created_at")] DateTimeOffset CreatedAt,
    [property: JsonPropertyName("state")] string? State = null)
{
    public string StateLabel => State switch
    {
        "archived" => "Archivé",
        "trashed" => "Corbeille",
        _ => "Actif",
    };
}
public sealed record ItemPageResponse(
    [property: JsonPropertyName("items")] List<InventoryItem> Items,
    [property: JsonPropertyName("next_cursor")] string? NextCursor);
public sealed record InventoryTransfer(
    [property: JsonPropertyName("id")] Guid Id,
    [property: JsonPropertyName("source_space_id")] Guid SourceSpaceId,
    [property: JsonPropertyName("destination_space_id")] Guid DestinationSpaceId,
    [property: JsonPropertyName("source_inventory_number")] string SourceInventoryNumber,
    [property: JsonPropertyName("destination_inventory_number")] string DestinationInventoryNumber,
    [property: JsonPropertyName("transferred_at")] DateTimeOffset TransferredAt);
public sealed record TransferOutcome(
    [property: JsonPropertyName("item")] InventoryItem Item,
    [property: JsonPropertyName("transfer")] InventoryTransfer Transfer);
public sealed record TransferListResponse([property: JsonPropertyName("transfers")] List<InventoryTransfer> Transfers);
public sealed record TransferDisplay(string Description, string Date);
public sealed record ItemStateAuditEvent(
    [property: JsonPropertyName("id")] Guid Id,
    [property: JsonPropertyName("actor_account_id")] Guid ActorAccountId,
    [property: JsonPropertyName("actor_display_name")] string ActorDisplayName,
    [property: JsonPropertyName("state_before")] string StateBefore,
    [property: JsonPropertyName("state_after")] string StateAfter,
    [property: JsonPropertyName("created_at")] DateTimeOffset CreatedAt)
{
    public string Description => $"{ActorDisplayName} : {StateLabel(StateBefore)} → {StateLabel(StateAfter)}";
    public string Date => CreatedAt.ToLocalTime().ToString("g");

    private static string StateLabel(string state) => state switch
    {
        "active" => "Actif",
        "archived" => "Archivé",
        "trashed" => "Corbeille",
        _ => state,
    };
}
public sealed record ItemStateEventListResponse(
    [property: JsonPropertyName("events")] List<ItemStateAuditEvent> Events);

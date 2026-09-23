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

    public bool IsSignedIn => _token is not null;
    public string? ServerAddress => _server?.ToString();
    public string? CurrentSessionId { get; private set; }
    public Guid? CurrentAccountId { get; private set; }

    public SessionApi(HttpMessageHandler? handler = null)
    {
        _client = new HttpClient(handler ?? new HttpClientHandler { AllowAutoRedirect = false })
        {
            Timeout = TimeSpan.FromSeconds(15),
        };
    }

    public void Configure(string address)
    {
        if (!Uri.TryCreate(address.Trim(), UriKind.Absolute, out var uri) ||
            !string.IsNullOrEmpty(uri.UserInfo) ||
            !string.IsNullOrEmpty(uri.Query) ||
            !string.IsNullOrEmpty(uri.Fragment) ||
            uri.AbsolutePath != "/" ||
            (uri.Scheme != Uri.UriSchemeHttps &&
             !(uri.Scheme == Uri.UriSchemeHttp && uri.IsLoopback)))
        {
            throw new ArgumentException("Utilisez une adresse HTTPS, ou HTTP pour un serveur local uniquement.");
        }

        // Changing servers must never send a token issued by the previous server.
        if (_server != uri)
        {
            SignOut();
        }

        _server = uri;
    }

    public async Task<string> CheckHealthAsync()
    {
        using var response = await _client.GetAsync(Endpoint("api/v1/health"));
        await EnsureSuccessAsync(response);
        var health = await response.Content.ReadFromJsonAsync<HealthResponse>();
        return health?.Status == "ok"
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

    public Task<CollectionSpace> CreateSpaceAsync(string name) =>
        SendJsonAsync<CollectionSpace>(HttpMethod.Post, "api/v1/spaces", new { name });

    public async Task<IReadOnlyList<SpaceInvitation>> GetInvitationsAsync(Guid spaceId)
    {
        var result = await SendJsonAsync<InvitationListResponse>(HttpMethod.Get, $"api/v1/spaces/{spaceId}/invitations");
        return result.Invitations;
    }

    public Task<SpaceInvitation> InviteAsync(Guid spaceId, string email) =>
        SendJsonAsync<SpaceInvitation>(HttpMethod.Post, $"api/v1/spaces/{spaceId}/invitations", new { email });

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

    public Task<ItemPageResponse> GetItemsPageAsync(Guid spaceId, string? search = null, string? after = null, int limit = 50)
    {
        if (limit is < 1 or > 100) throw new ArgumentOutOfRangeException(nameof(limit));
        var parameters = new List<string> { $"limit={limit}" };
        if (!string.IsNullOrWhiteSpace(search)) parameters.Add($"q={Uri.EscapeDataString(search.Trim())}");
        if (!string.IsNullOrEmpty(after)) parameters.Add($"after={Uri.EscapeDataString(after)}");
        return SendJsonAsync<ItemPageResponse>(HttpMethod.Get,
            $"api/v1/spaces/{spaceId}/items?{string.Join("&", parameters)}");
    }

    public Task<InventoryItem> CreateItemAsync(Guid spaceId, string name) =>
        SendJsonAsync<InventoryItem>(HttpMethod.Post, $"api/v1/spaces/{spaceId}/items", new { name });

    public Task<InventoryItem> GetItemAsync(Guid itemId) =>
        SendJsonAsync<InventoryItem>(HttpMethod.Get, $"api/v1/items/{itemId}");

    public Task<TransferOutcome> TransferItemAsync(Guid itemId, Guid destinationSpaceId, string expectedRevision) =>
        SendJsonAsync<TransferOutcome>(HttpMethod.Post, $"api/v1/items/{itemId}/transfers",
            new { destination_space_id = destinationSpaceId, expected_revision = expectedRevision });

    public async Task<IReadOnlyList<InventoryTransfer>> GetItemTransfersAsync(Guid itemId)
    {
        var result = await SendJsonAsync<TransferListResponse>(HttpMethod.Get, $"api/v1/items/{itemId}/transfers");
        return result.Transfers;
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
public sealed record SessionPrincipal([property: JsonPropertyName("subject")] Guid Subject);
public sealed record HealthResponse([property: JsonPropertyName("status")] string Status);
public sealed record ProblemResponse([property: JsonPropertyName("detail")] string? Detail);
public sealed record CollectionSpace(
    [property: JsonPropertyName("id")] Guid Id,
    [property: JsonPropertyName("name")] string Name,
    [property: JsonPropertyName("owner_account_id")] Guid OwnerAccountId);
public sealed record SpaceListResponse([property: JsonPropertyName("spaces")] List<CollectionSpace> Spaces);
public sealed record SpaceInvitation(
    [property: JsonPropertyName("id")] Guid Id,
    [property: JsonPropertyName("recipient_email")] string RecipientEmail,
    [property: JsonPropertyName("expires_at")] DateTimeOffset ExpiresAt,
    [property: JsonPropertyName("accepted_at")] DateTimeOffset? AcceptedAt,
    [property: JsonPropertyName("revoked_at")] DateTimeOffset? RevokedAt)
{
    public string Status => AcceptedAt is not null ? "Acceptée" : RevokedAt is not null ? "Révoquée" : ExpiresAt <= DateTimeOffset.UtcNow ? "Expirée" : "En attente";
    public bool IsPending => AcceptedAt is null && RevokedAt is null && ExpiresAt > DateTimeOffset.UtcNow;
}
public sealed record InvitationListResponse([property: JsonPropertyName("invitations")] List<SpaceInvitation> Invitations);
public sealed record InventoryItem(
    [property: JsonPropertyName("id")] Guid Id,
    [property: JsonPropertyName("space_id")] Guid SpaceId,
    [property: JsonPropertyName("inventory_number")] string InventoryNumber,
    [property: JsonPropertyName("name")] string Name,
    [property: JsonPropertyName("revision")] string Revision);
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

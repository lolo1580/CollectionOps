using System.Net;
using System.Text;
using CollectionOps.Client.Services;

await RejectInsecureRemoteServer();
await RejectUnexpectedHealth();
await ClearExpiredSession();
await RejectMalformedJson();
Console.WriteLine("SessionApi: 4 checks passed.");

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

    public void Enqueue(HttpStatusCode status, string body) => _responses.Enqueue(new HttpResponseMessage(status)
    {
        Content = new StringContent(body, Encoding.UTF8, "application/json"),
    });

    protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken)
    {
        LastToken = request.Headers.TryGetValues("x-session-token", out var values)
            ? values.Single()
            : null;
        return Task.FromResult(_responses.Dequeue());
    }
}

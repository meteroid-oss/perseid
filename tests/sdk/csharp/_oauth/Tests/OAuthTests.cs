using System.Net;
using System.Text;
using Vault;

/// <summary>The OAuth2 client credentials flow of the SDK generated from tests/fixtures/oauth.yaml,
/// against a handler that plays the token endpoint and the API.</summary>
public class OAuthTests
{
    private const string BaseUrl = "https://vault.test/v1";
    private const string TokenUrl = "https://vault.test/v1/oauth/token";

    /// <summary>Issues the access tokens <c>at-1</c>, <c>at-2</c>... and rejects those in
    /// <see cref="Revoked"/> with a 401.</summary>
    private sealed class Server : HttpMessageHandler
    {
        private int _issued;

        public List<(HttpRequestMessage Request, string Body)> Seen { get; } = [];

        public HashSet<string> Revoked { get; } = [];

        public Queue<HttpResponseMessage> Scripted { get; } = new();

        public string ExpiresIn { get; set; } = "3600";

        public TimeSpan TokenDelay { get; set; }

        public List<(HttpRequestMessage Request, string Body)> Tokens =>
            Seen.Where(seen => seen.Request.RequestUri!.ToString() == TokenUrl).ToList();

        protected override async Task<HttpResponseMessage> SendAsync(
            HttpRequestMessage request,
            CancellationToken cancellationToken
        )
        {
            var body = request.Content is null ? "" : await request.Content.ReadAsStringAsync(cancellationToken);
            lock (Seen)
            {
                Seen.Add((request, body));
            }
            if (request.RequestUri!.ToString() == TokenUrl)
            {
                await Task.Delay(TokenDelay, cancellationToken);
                lock (Scripted)
                {
                    if (Scripted.Count > 0)
                    {
                        return Scripted.Dequeue();
                    }
                }
                var n = Interlocked.Increment(ref _issued);
                return Reply(
                    HttpStatusCode.OK,
                    $$"""{"access_token":"at-{{n}}","token_type":"Bearer","expires_in":{{ExpiresIn}}}"""
                );
            }
            var token = request.Headers.Authorization?.Parameter ?? "anonymous";
            return Revoked.Contains(token)
                ? Reply(HttpStatusCode.Unauthorized, """{"message":"revoked"}""")
                : Reply(HttpStatusCode.OK, $$"""{"status":"{{token}}"}""");
        }
    }

    private static HttpResponseMessage Reply(HttpStatusCode status, string body) =>
        new(status) { Content = new StringContent(body, Encoding.UTF8, "application/json") };

    private static VaultClient Client(
        Server server,
        string? token = null,
        Action<VaultClientOptions>? configure = null
    )
    {
        var options = new VaultClientOptions
        {
            BaseUrl = BaseUrl,
            ClientId = "id",
            ClientSecret = "secret",
            MaxRetries = 0,
            HttpMessageHandler = server,
        };
        configure?.Invoke(options);
        return new VaultClient(token, options);
    }

    private static string Basic(string id, string secret) =>
        Convert.ToBase64String(Encoding.UTF8.GetBytes($"{id}:{secret}"));

    [Fact]
    public async Task ATokenIsFetchedOnFirstUseAndKept()
    {
        var server = new Server();
        using var client = Client(server);
        Assert.Equal("at-1", (await client.Account.RetrieveMachineAsync()).Status);
        Assert.Equal("at-1", (await client.Account.RetrieveMachineAsync()).Status);
        var token = Assert.Single(server.Tokens);
        Assert.Equal(HttpMethod.Post, token.Request.Method);
        Assert.Equal("application/x-www-form-urlencoded", token.Request.Content!.Headers.ContentType!.MediaType);
        Assert.Equal("Basic", token.Request.Headers.Authorization!.Scheme);
        Assert.Equal(Basic("id", "secret"), token.Request.Headers.Authorization!.Parameter);
        Assert.Equal(
            "grant_type=client_credentials&scope=secrets.read+secrets.write",
            token.Body
        );
        Assert.Equal("Bearer at-1", server.Seen[1].Request.Headers.Authorization!.ToString());
    }

    [Fact]
    public async Task PublicOperationsSendNoCredentialsAndFetchNoToken()
    {
        var server = new Server();
        using var client = Client(server);
        Assert.Equal("anonymous", (await client.Account.CheckHealthAsync()).Status);
        Assert.Empty(server.Tokens);
        Assert.Null(server.Seen[0].Request.Headers.Authorization);
        Assert.Equal("at-1", (await client.Account.CreateSessionAsync()).Status);
    }

    [Fact]
    public async Task CredentialsAreFormEncodedBeforeTheBasicHeaderOrSentInTheBody()
    {
        var odd = new Server();
        using (var client = Client(odd, configure: o => (o.ClientId, o.ClientSecret) = ("a b", "p@ss:word")))
        {
            await client.Account.RetrieveMachineAsync();
        }
        Assert.Equal(Basic("a+b", "p%40ss%3Aword"), odd.Tokens[0].Request.Headers.Authorization!.Parameter);

        var inBody = new Server();
        using (var client = Client(inBody, configure: o => o.OAuthClientAuthInBody = true))
        {
            await client.Account.RetrieveMachineAsync();
        }
        var token = inBody.Tokens[0];
        Assert.Null(token.Request.Headers.Authorization);
        Assert.Contains("client_id=id&client_secret=secret", token.Body);
    }

    [Fact]
    public async Task AnExpiredTokenIsRenewed()
    {
        var server = new Server { ExpiresIn = "0" };
        using var client = Client(server);
        Assert.Equal("at-1", (await client.Account.RetrieveMachineAsync()).Status);
        Assert.Equal("at-2", (await client.Account.RetrieveMachineAsync()).Status);
        Assert.Equal(2, server.Tokens.Count);
    }

    [Fact]
    public async Task ATokenTheApiRejectsIsReplacedOnce()
    {
        var server = new Server { Revoked = { "at-1" } };
        using (var client = Client(server))
        {
            Assert.Equal("at-2", (await client.Account.RetrieveMachineAsync()).Status);
            Assert.Equal(2, server.Tokens.Count);
            Assert.Equal(2, server.Seen.Count - server.Tokens.Count);
            Assert.Equal("at-2", (await client.Account.RetrieveMachineAsync()).Status);
            Assert.Equal(2, server.Tokens.Count);
        }

        var always = new Server { Revoked = { "at-1", "at-2", "at-3" } };
        using (var client = Client(always))
        {
            await Assert.ThrowsAsync<UnauthorizedException>(() => client.Account.RetrieveMachineAsync());
        }
        Assert.Equal(2, always.Tokens.Count);
    }

    [Fact]
    public async Task TheTokenRequestIsRetriedLikeAnyRequest()
    {
        var busy = new Server();
        busy.Scripted.Enqueue(Reply(HttpStatusCode.ServiceUnavailable, "{}"));
        using (var client = Client(busy, configure: o => o.RetrySchedule = [TimeSpan.Zero]))
        {
            Assert.Equal("at-1", (await client.Account.RetrieveMachineAsync()).Status);
        }
        Assert.Equal(2, busy.Tokens.Count);

        var invalid = new Server();
        invalid.Scripted.Enqueue(Reply(HttpStatusCode.Unauthorized, """{"error":"invalid_client"}"""));
        using (var client = Client(invalid))
        {
            var error = await Assert.ThrowsAsync<UnauthorizedException>(() => client.Account.RetrieveMachineAsync());
            Assert.Contains("invalid_client", error.Message + error.Error);
        }
        Assert.Single(invalid.Seen);

        var empty = new Server();
        empty.Scripted.Enqueue(Reply(HttpStatusCode.OK, """{"token_type":"Bearer"}"""));
        using (var client = Client(empty))
        {
            await Assert.ThrowsAsync<ApiDecodeException>(() => client.Account.RetrieveMachineAsync());
        }
    }

    [Fact]
    public async Task ConcurrentCallsShareOneTokenRequest()
    {
        var server = new Server { TokenDelay = TimeSpan.FromMilliseconds(100) };
        using var client = Client(server);
        var results = await Task.WhenAll(Enumerable.Range(0, 4).Select(_ => client.Account.RetrieveMachineAsync()));
        Assert.All(results, health => Assert.Equal("at-1", health.Status));
        Assert.Single(server.Tokens);
    }

    [Fact]
    public async Task ATokenOrAProviderWinsOverTheClientCredentials()
    {
        var server = new Server();
        using (var client = Client(server, configure: o => o.TokenProvider = _ => ValueTask.FromResult("mine")))
        {
            Assert.Equal("mine", (await client.Account.RetrieveMachineAsync()).Status);
        }
        using (var client = Client(server, token: "static"))
        {
            Assert.Equal("static", (await client.Account.RetrieveMachineAsync()).Status);
        }
        Assert.Empty(server.Tokens);
    }

    [Fact]
    public async Task TheCredentialsDefaultToTheEnvironment()
    {
        Environment.SetEnvironmentVariable("VAULT_CLIENT_ID", "env-id");
        Environment.SetEnvironmentVariable("VAULT_CLIENT_SECRET", "env-secret");
        try
        {
            var server = new Server();
            using var client = new VaultClient(
                null,
                new VaultClientOptions { BaseUrl = BaseUrl, HttpMessageHandler = server, MaxRetries = 0 }
            );
            Assert.Equal("at-1", (await client.Account.RetrieveMachineAsync()).Status);
            Assert.Equal(Basic("env-id", "env-secret"), server.Tokens[0].Request.Headers.Authorization!.Parameter);
        }
        finally
        {
            Environment.SetEnvironmentVariable("VAULT_CLIENT_ID", null);
            Environment.SetEnvironmentVariable("VAULT_CLIENT_SECRET", null);
        }
    }
}

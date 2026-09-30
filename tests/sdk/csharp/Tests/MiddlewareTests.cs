using System.Collections.Concurrent;
using System.Net;
using System.Text;
using Petstore;

public class MiddlewareTests
{
    private const string PetJson =
        """{"id":"1","name":"Rex","created_at":"2024-01-01T00:00:00Z"}""";

    private sealed class Origin : HttpMessageHandler
    {
        public List<HttpRequestMessage> Seen { get; } = [];

        protected override Task<HttpResponseMessage> SendAsync(
            HttpRequestMessage request,
            CancellationToken cancellationToken
        )
        {
            Seen.Add(request);
            return Task.FromResult(Json(PetJson));
        }
    }

    private sealed class Cache : DelegatingHandler
    {
        private readonly ConcurrentDictionary<string, string> _store = new();

        protected override async Task<HttpResponseMessage> SendAsync(
            HttpRequestMessage request,
            CancellationToken cancellationToken
        )
        {
            var key = request.RequestUri!.ToString();
            if (request.Method != HttpMethod.Get)
            {
                return await base.SendAsync(request, cancellationToken);
            }
            if (_store.TryGetValue(key, out var hit))
            {
                return Json(hit);
            }
            var response = await base.SendAsync(request, cancellationToken);
            if (response.IsSuccessStatusCode)
            {
                _store[key] = await response.Content.ReadAsStringAsync(cancellationToken);
            }
            return response;
        }
    }

    private sealed class Tag(string name, List<string> order) : DelegatingHandler
    {
        protected override Task<HttpResponseMessage> SendAsync(
            HttpRequestMessage request,
            CancellationToken cancellationToken
        )
        {
            order.Add(name);
            request.Headers.Add("x-tag", name);
            return base.SendAsync(request, cancellationToken);
        }
    }

    private static HttpResponseMessage Json(string body) =>
        new(HttpStatusCode.OK)
        {
            Content = new StringContent(body, Encoding.UTF8, "application/json"),
        };

    [Fact]
    public async Task ACacheAnswersRepeatedGetsWithoutReachingTheOrigin()
    {
        var origin = new Origin();
        var options = new PetstoreClientOptions { HttpMessageHandler = origin };
        options.Handlers.Add(new Cache());
        using var client = new PetstoreClient("token", options);
        for (var i = 0; i < 3; i++)
        {
            Assert.Equal("Rex", (await client.Pets.GetPetAsync("1")).Name);
        }
        Assert.Single(origin.Seen);
        await client.Pets.GetPetAsync("2");
        Assert.Equal(2, origin.Seen.Count);
    }

    [Fact]
    public async Task MiddlewareRunsInOrderAndCanChangeTheRequest()
    {
        var origin = new Origin();
        var order = new List<string>();
        var options = new PetstoreClientOptions { HttpMessageHandler = origin };
        options.Handlers.Add(new Tag("outer", order));
        options.Handlers.Add(new Tag("inner", order));
        using var client = new PetstoreClient("token", options);
        await client.Pets.GetPetAsync("1");
        Assert.Equal(["outer", "inner"], order);
        Assert.Equal(["outer", "inner"], origin.Seen[0].Headers.GetValues("x-tag"));
        Assert.Equal("Bearer token", origin.Seen[0].Headers.Authorization?.ToString());
    }

    [Fact]
    public async Task MiddlewareRunsInsideTheRetryLoop()
    {
        var attempts = 0;
        var origin = new Stub(_ =>
            ++attempts == 1 ? new HttpResponseMessage(HttpStatusCode.ServiceUnavailable) : Json(PetJson)
        );
        var order = new List<string>();
        var options = new PetstoreClientOptions
        {
            HttpMessageHandler = origin,
            RetrySchedule = [TimeSpan.Zero],
        };
        options.Handlers.Add(new Tag("tag", order));
        using var client = new PetstoreClient("token", options);
        await client.Pets.GetPetAsync("1");
        Assert.Equal(2, order.Count);
    }

    [Fact]
    public async Task MiddlewareComposesWithACallersHttpClient()
    {
        var origin = new Origin();
        using var http = new HttpClient(origin);
        var options = new PetstoreClientOptions();
        options.Handlers.Add(new Cache());
        using (var client = new PetstoreClient(http, "token", options))
        {
            await client.Pets.GetPetAsync("1");
            await client.Pets.GetPetAsync("1");
        }
        Assert.Single(origin.Seen);
        await http.GetAsync("https://example.com/still-usable");
        Assert.Equal(2, origin.Seen.Count);
    }

    private sealed class Stub(Func<HttpRequestMessage, HttpResponseMessage> respond)
        : HttpMessageHandler
    {
        protected override Task<HttpResponseMessage> SendAsync(
            HttpRequestMessage request,
            CancellationToken cancellationToken
        ) => Task.FromResult(respond(request));
    }
}

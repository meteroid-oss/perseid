using System.Diagnostics;
using System.Net;
using System.Text;
using Torture;
using Torture.Models;

public class HttpTests
{
    private const string ThingJson = """
        {"id":"t1","name":"n","created_at":"2024-01-02T03:04:05Z","kind":"alpha","count":1,
        "nullable_required":null,"tags":[],"metadata":{},"attrs":{}}
        """;

    private sealed class Server(Func<int, HttpRequestMessage, CancellationToken, Task<HttpResponseMessage>> respond)
        : HttpMessageHandler
    {
        public List<(HttpRequestMessage Request, string Body)> Seen { get; } = [];

        protected override async Task<HttpResponseMessage> SendAsync(
            HttpRequestMessage request,
            CancellationToken cancellationToken
        )
        {
            var body = request.Content is null ? "" : await request.Content.ReadAsStringAsync(cancellationToken);
            Seen.Add((request, body));
            return await respond(Seen.Count, request, cancellationToken);
        }
    }

    private static HttpResponseMessage Reply(HttpStatusCode status, string body = "{}") =>
        new(status) { Content = new StringContent(body, Encoding.UTF8, "application/json") };

    private static (TortureClient, Server) Client(
        Func<int, HttpResponseMessage> respond,
        TortureClientOptions? options = null
    )
    {
        var server = new Server((n, _, _) => Task.FromResult(respond(n)));
        options ??= new TortureClientOptions { RetrySchedule = [TimeSpan.Zero, TimeSpan.Zero] };
        options.BaseUrl = "https://torture.test/v1";
        options.HttpMessageHandler = server;
        return (new TortureClient("token", options), server);
    }

    private static string? Header(HttpRequestMessage request, string name) =>
        request.Headers.TryGetValues(name, out var values) ? string.Join(",", values) : null;

    [Fact]
    public async Task KeepsTheBasePathAndEscapesPathParameters()
    {
        var (client, server) = Client(_ => Reply(HttpStatusCode.OK, ThingJson));
        using var _ = client;
        var thing = await client.Things.UpdateThingAsync("a/b", new ThingPatch { Count = 9007199254740993L });
        Assert.Equal("/v1/things/a%2Fb", server.Seen[0].Request.RequestUri!.AbsolutePath);
        Assert.Equal("""{"count":9007199254740993}""", server.Seen[0].Body);
        Assert.Equal("t1", thing.Id);
    }

    [Fact]
    public async Task SendsQueryParameters()
    {
        var (client, server) = Client(_ => Reply(HttpStatusCode.OK, """{"data":[]}"""));
        using var _ = client;
        await client.Things.ListThingsAsync(
            new()
            {
                Ids = ["a", "b"],
                CsvIds = ["c", "d"],
                Kind = Kind.Beta2,
                Since = new DateTimeOffset(2024, 1, 2, 3, 4, 5, TimeSpan.FromHours(1)),
                Big = 9007199254740993L,
                Flag = true,
                Ratio = 0.5,
                XRequired = "r",
            }
        );
        var request = server.Seen[0].Request;
        Assert.Equal(
            "?ids=a&ids=b&csv_ids=c%2Cd&kind=beta-2&since=2024-01-02T03%3A04%3A05.0000000%2B01%3A00&big=9007199254740993&flag=true&ratio=0.5",
            request.RequestUri!.Query
        );
        Assert.Equal("r", Header(request, "X-Required"));
    }

    [Fact]
    public async Task SendsOneIdempotencyKeyTheCallersFirst()
    {
        var (client, server) = Client(_ => Reply(HttpStatusCode.Created, ThingJson));
        using var _ = client;
        var body = new ThingCreate { Name = "n", Kind = Kind.Alpha };
        await client.Things.CreateThingAsync(body, new() { IdempotencyKey = "mine" });
        await client.Things.CreateThingAsync(
            body,
            requestOptions: new() { Headers = { ["IDEMPOTENCY-KEY"] = "theirs" } }
        );
        await client.Things.CreateThingAsync(body, requestOptions: new() { IdempotencyKey = "option" });
        await client.Things.CreateThingAsync(body);
        Assert.Equal("mine", Header(server.Seen[0].Request, "Idempotency-Key"));
        Assert.Equal("theirs", Header(server.Seen[1].Request, "Idempotency-Key"));
        Assert.Equal("option", Header(server.Seen[2].Request, "Idempotency-Key"));
        Assert.Matches("^auto_[0-9a-f-]{36}$", Header(server.Seen[3].Request, "Idempotency-Key"));
    }

    [Fact]
    public async Task TakesPerCallHeaders()
    {
        var (client, server) = Client(_ => Reply(HttpStatusCode.OK, ThingJson));
        using var _ = client;
        await client.Things.GetThingAsync(
            "t1",
            new RequestOptions { Headers = { ["X-Extra"] = "1", ["User-Agent"] = "mine" } }
        );
        var request = server.Seen[0].Request;
        Assert.Equal("1", Header(request, "X-Extra"));
        Assert.Equal("mine", Header(request, "User-Agent"));
        Assert.Equal("Bearer token", Header(request, "Authorization"));
    }

    [Theory]
    [InlineData(HttpStatusCode.TooManyRequests, 3)]
    [InlineData(HttpStatusCode.RequestTimeout, 3)]
    [InlineData(HttpStatusCode.BadGateway, 3)]
    [InlineData(HttpStatusCode.Conflict, 1)]
    [InlineData(HttpStatusCode.BadRequest, 1)]
    public async Task RetriesTransientFailures(HttpStatusCode status, int attempts)
    {
        var (client, server) = Client(_ => Reply(status));
        using var _ = client;
        var error = await Assert.ThrowsAnyAsync<ApiException>(() => client.Things.GetThingAsync("t1"));
        Assert.Equal(status, error.StatusCode);
        Assert.Equal(attempts, server.Seen.Count);
        if (attempts > 1)
        {
            Assert.Equal("2", Header(server.Seen[^1].Request, "torture-retry-count"));
        }
    }

    [Fact]
    public async Task DoesNotRetryAPatchWithoutAnIdempotencyKey()
    {
        var (client, server) = Client(_ => Reply(HttpStatusCode.InternalServerError));
        using var _ = client;
        await Assert.ThrowsAnyAsync<ApiException>(() => client.Things.UpdateThingAsync("t1", new()));
        Assert.Single(server.Seen);
        await Assert.ThrowsAnyAsync<ApiException>(
            () => client.Things.UpdateThingAsync("t1", new(), new() { IdempotencyKey = "k" })
        );
        Assert.Equal(4, server.Seen.Count);
    }

    [Fact]
    public async Task HonorsRetryAfter()
    {
        var (client, server) = Client(
            n =>
            {
                if (n > 1)
                {
                    return Reply(HttpStatusCode.OK, ThingJson);
                }
                var response = Reply(HttpStatusCode.TooManyRequests);
                response.Headers.TryAddWithoutValidation("Retry-After", "1");
                return response;
            },
            new() { RetrySchedule = [TimeSpan.FromHours(1)] }
        );
        using var _ = client;
        var clock = Stopwatch.StartNew();
        await client.Things.GetThingAsync("t1");
        Assert.InRange(clock.Elapsed, TimeSpan.FromSeconds(0.9), TimeSpan.FromSeconds(30));
        Assert.Equal(2, server.Seen.Count);
    }

    [Fact]
    public async Task PerCallMaxRetriesAndTimeout()
    {
        var server = new Server(async (_, _, ct) =>
        {
            await Task.Delay(Timeout.Infinite, ct);
            return Reply(HttpStatusCode.OK);
        });
        using var client = new TortureClient(
            "token",
            new() { HttpMessageHandler = server, RetrySchedule = [TimeSpan.Zero] }
        );
        await Assert.ThrowsAsync<TimeoutException>(
            () => client.Things.GetThingAsync("t1", new() { Timeout = TimeSpan.FromMilliseconds(20) })
        );
        Assert.Equal(2, server.Seen.Count);
        await Assert.ThrowsAsync<TimeoutException>(
            () =>
                client.Things.GetThingAsync(
                    "t1",
                    new() { Timeout = TimeSpan.FromMilliseconds(20), MaxRetries = 0 }
                )
        );
        Assert.Equal(3, server.Seen.Count);
    }

    [Fact]
    public async Task CancellationStopsRetries()
    {
        using var cancel = new CancellationTokenSource();
        var (client, server) = Client(_ =>
        {
            cancel.Cancel();
            return Reply(HttpStatusCode.ServiceUnavailable);
        });
        using var _ = client;
        await Assert.ThrowsAnyAsync<OperationCanceledException>(
            () => client.Things.GetThingAsync("t1", cancellationToken: cancel.Token)
        );
        Assert.Single(server.Seen);
    }

    [Fact]
    public async Task ReturnsBinaryAndTextBodies()
    {
        var (client, _) = Client(_ =>
            new HttpResponseMessage(HttpStatusCode.OK) { Content = new ByteArrayContent([1, 2, 3]) }
        );
        using var _ = client;
        Assert.Equal([1, 2, 3], await client.Things.DownloadThingAsync("t1"));
    }

    [Fact]
    public async Task ErrorsAreThrownAsTheirStatusClassWithATypedBody()
    {
        var (client, _) = Client(n =>
        {
            var response = Reply(
                n == 1 ? HttpStatusCode.UnprocessableEntity : HttpStatusCode.NotFound,
                """{"message":"invalid","fields":{"name":["empty"]}}"""
            );
            response.Headers.TryAddWithoutValidation("x-request-id", "req_1");
            return response;
        });
        using var _ = client;
        var invalid = await Assert.ThrowsAsync<UnprocessableEntityException>(
            () => client.Things.CreateThingAsync(new ThingCreate { Name = "", Kind = Kind.Alpha })
        );
        Assert.Equal(["empty"], invalid.GetError<ValidationError>()!.Fields!["name"]);
        Assert.Equal("req_1", invalid.GetRequestId());
        var missing = await Assert.ThrowsAsync<NotFoundException>(() => client.Things.GetThingAsync("t1"));
        Assert.IsAssignableFrom<ApiException>(missing);
        Assert.Null(missing.GetError<Problem>());
    }
}

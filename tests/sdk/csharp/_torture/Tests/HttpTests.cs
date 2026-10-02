using System.Diagnostics;
using System.Net;
using System.Text;
using System.Text.Json;
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
    public async Task WithoutAServerTheBaseUrlIsRequired()
    {
        var error = Assert.Throws<TortureException>(() => new TortureClient("token"));
        Assert.Contains("BaseUrl", error.Message);
        Assert.Contains("TORTURE_BASE_URL", error.Message);
        Environment.SetEnvironmentVariable("TORTURE_BASE_URL", "https://env.test/v2");
        try
        {
            var server = new Server((_, _, _) => Task.FromResult(Reply(HttpStatusCode.OK, ThingJson)));
            using var client = new TortureClient("token", new() { HttpMessageHandler = server });
            await client.Things.RetrieveAsync("t1");
            Assert.Equal("https://env.test/v2/things/t1", server.Seen[0].Request.RequestUri!.ToString());
        }
        finally
        {
            Environment.SetEnvironmentVariable("TORTURE_BASE_URL", null);
        }
    }

    [Fact]
    public async Task KeepsTheBasePathAndEscapesPathParameters()
    {
        var (client, server) = Client(_ => Reply(HttpStatusCode.OK, ThingJson));
        using var _ = client;
        var thing = await client.Things.UpdateAsync("a/b", new ThingPatch { Count = 9007199254740993L });
        Assert.Equal("/v1/things/a%2Fb", server.Seen[0].Request.RequestUri!.AbsolutePath);
        Assert.Equal("""{"count":9007199254740993}""", server.Seen[0].Body);
        Assert.Equal("t1", thing.Id);
    }

    [Fact]
    public async Task SendsQueryParameters()
    {
        var (client, server) = Client(_ => Reply(HttpStatusCode.OK, """{"data":[]}"""));
        using var _ = client;
        await client.Things.ListAsync(
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
        await client.Things.CreateAsync(body, new() { IdempotencyKey = "mine" });
        await client.Things.CreateAsync(
            body,
            requestOptions: new() { Headers = { ["IDEMPOTENCY-KEY"] = "theirs" } }
        );
        await client.Things.CreateAsync(body, requestOptions: new() { IdempotencyKey = "option" });
        await client.Things.CreateAsync(body);
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
        await client.Things.RetrieveAsync(
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
    [InlineData(HttpStatusCode.NotImplemented, 3)]
    [InlineData(HttpStatusCode.Conflict, 1)]
    [InlineData(HttpStatusCode.BadRequest, 1)]
    public async Task RetriesTransientFailures(HttpStatusCode status, int attempts)
    {
        var (client, server) = Client(_ => Reply(status));
        using var _ = client;
        var error = await Assert.ThrowsAnyAsync<ApiException>(() => client.Things.RetrieveAsync("t1"));
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
        await Assert.ThrowsAnyAsync<ApiException>(() => client.Things.UpdateAsync("t1", new()));
        Assert.Single(server.Seen);
        await Assert.ThrowsAnyAsync<ApiException>(
            () => client.Things.UpdateAsync("t1", new(), new() { IdempotencyKey = "k" })
        );
        Assert.Equal(4, server.Seen.Count);
    }

    [Fact]
    public async Task DoesNotRetryARateLimitedPatchWithoutAnIdempotencyKey()
    {
        var (client, server) = Client(_ => Reply(HttpStatusCode.TooManyRequests));
        using var _ = client;
        await Assert.ThrowsAsync<RateLimitException>(() => client.Things.UpdateAsync("t1", new()));
        Assert.Single(server.Seen);
    }

    [Fact]
    public async Task BacksOffInsteadOfAFarRetryAfter()
    {
        var (client, server) = Client(n =>
        {
            var response = n > 2 ? Reply(HttpStatusCode.OK, ThingJson) : Reply(HttpStatusCode.ServiceUnavailable);
            response.Headers.TryAddWithoutValidation("Retry-After", n == 1 ? "3600" : "-5");
            return response;
        });
        using var _ = client;
        var clock = Stopwatch.StartNew();
        await client.Things.RetrieveAsync("t1");
        Assert.Equal(3, server.Seen.Count);
        Assert.InRange(clock.Elapsed, TimeSpan.Zero, TimeSpan.FromSeconds(30));
    }

    [Fact]
    public async Task StreamTwinsSetStreamWithoutTouchingTheBody()
    {
        var (client, server) = Client(_ =>
            new HttpResponseMessage(HttpStatusCode.OK)
            {
                Content = new StringContent("data: {\"text\":\"hi\"}\n\ndata: [DONE]\n\n", Encoding.UTF8, "text/event-stream"),
            }
        );
        using var _ = client;
        var texts = new List<string>();
        await foreach (var reply in await client.Chats.CreateStreamAsync())
        {
            texts.Add(reply.Text);
        }
        var body = new ChatRequest { Model = "m" };
        await using (await client.Chats.CreateStreamAsync(body)) { }
        Assert.Equal(["hi"], texts);
        Assert.Equal("""{"stream":true}""", server.Seen[0].Body);
        Assert.Equal("""{"model":"m","stream":true}""", server.Seen[1].Body);
        Assert.Null(body.Stream);
    }

    [Fact]
    public async Task ABodilessSuccessIsNullWhereTheSpecAllowsIt()
    {
        var (client, _) = Client(n =>
            n == 1
                ? new HttpResponseMessage(HttpStatusCode.Accepted)
                : Reply(HttpStatusCode.OK, """{"id":"j1"}""")
        );
        using var _ = client;
        Assert.Null(await client.Jobs.RetrieveAsync("j1"));
        Assert.Equal("j1", (await client.Jobs.RetrieveAsync("j1"))?.Id);
    }

    [Fact]
    public async Task HonorsRetryAfterMilliseconds()
    {
        var (client, server) = Client(
            n =>
            {
                if (n > 1)
                {
                    return Reply(HttpStatusCode.OK, ThingJson);
                }
                var response = Reply(HttpStatusCode.TooManyRequests);
                response.Headers.TryAddWithoutValidation("retry-after-ms", "1");
                return response;
            },
            new() { RetrySchedule = [TimeSpan.FromHours(1)] }
        );
        using var _ = client;
        await client.Things.RetrieveAsync("t1");
        Assert.Equal(2, server.Seen.Count);
    }

    [Fact]
    public async Task ConnectionFailuresAreRetriedThenWrapped()
    {
        var server = new Server((_, _, _) => throw new HttpRequestException("refused"));
        using var client = new TortureClient(
            "token",
            new()
            {
                BaseUrl = "https://torture.test/v1",
                HttpMessageHandler = server,
                RetrySchedule = [TimeSpan.Zero],
            }
        );
        var error = await Assert.ThrowsAsync<ApiConnectionException>(() => client.Things.RetrieveAsync("t1"));
        Assert.IsType<HttpRequestException>(error.InnerException);
        Assert.IsAssignableFrom<TortureException>(error);
        Assert.Equal(2, server.Seen.Count);
    }

    [Fact]
    public async Task UndecodableResponsesThrowADecodeError()
    {
        var (client, _) = Client(_ => Reply(HttpStatusCode.OK, "not json"));
        using var _ = client;
        var error = await Assert.ThrowsAsync<ApiDecodeException>(() => client.Things.RetrieveAsync("t1"));
        Assert.IsAssignableFrom<TortureException>(error);
    }

    [Fact]
    public async Task RawResponsesCarryTheStatusAndHeaders()
    {
        var (client, _) = Client(_ =>
        {
            var response = Reply(HttpStatusCode.OK, ThingJson);
            response.Headers.TryAddWithoutValidation("x-request-id", "req_1");
            return response;
        });
        using var _ = client;
        var raw = await client.Things.WithRawResponse.RetrieveAsync("t1");
        Assert.Equal(HttpStatusCode.OK, raw.StatusCode);
        Assert.Equal("req_1", raw.RequestId);
        Assert.Equal("application/json", raw.ContentHeaders!.ContentType!.MediaType);
        Assert.Equal("t1", raw.Value.Id);
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
        await client.Things.RetrieveAsync("t1");
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
            new()
            {
                BaseUrl = "https://torture.test/v1",
                HttpMessageHandler = server,
                RetrySchedule = [TimeSpan.Zero],
            }
        );
        await Assert.ThrowsAsync<ApiTimeoutException>(
            () => client.Things.RetrieveAsync("t1", new() { Timeout = TimeSpan.FromMilliseconds(20) })
        );
        Assert.Equal(2, server.Seen.Count);
        await Assert.ThrowsAsync<ApiTimeoutException>(
            () =>
                client.Things.RetrieveAsync(
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
            () => client.Things.RetrieveAsync("t1", cancellationToken: cancel.Token)
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
        Assert.Equal([1, 2, 3], await client.Things.DownloadAsync("t1"));
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
            () => client.Things.CreateAsync(new ThingCreate { Name = "", Kind = Kind.Alpha })
        );
        Assert.Equal(["empty"], invalid.GetError<ValidationError>()!.Fields!["name"]);
        Assert.Equal(["empty"], Assert.IsType<ValidationError>(invalid.Error).Fields!["name"]);
        Assert.Same(invalid.Error, invalid.Error);
        Assert.Equal("req_1", invalid.RequestId);
        var missing = await Assert.ThrowsAsync<NotFoundException>(() => client.Things.RetrieveAsync("t1"));
        Assert.IsAssignableFrom<TortureException>(missing);
        Assert.Null(missing.GetError<Problem>());
        Assert.Equal("invalid", ((JsonElement)missing.Error!).GetProperty("message").GetString());
    }
}

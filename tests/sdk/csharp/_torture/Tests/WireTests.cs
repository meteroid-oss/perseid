using System.Diagnostics;
using System.Globalization;
using System.Net;
using System.Net.Sockets;
using System.Text;
using System.Text.Json;
using Torture;
using Torture.Models;
using Reply = RawServer.Reply;

/// <summary>
/// A loopback HTTP/1.1 server over raw sockets, to see the request line, headers and body exactly as
/// the SDK sent them. Every request gets its own connection, closed after the reply.
/// </summary>
internal sealed class RawServer : IDisposable
{
    /// <summary>One request as it came in.</summary>
    public sealed record Request(
        int Number,
        string Line,
        IReadOnlyDictionary<string, string> Headers,
        string Body
    );

    /// <summary>What to answer, after an optional delay.</summary>
    public sealed record Reply(
        int Status = 200,
        string Body = "{}",
        IReadOnlyDictionary<string, string>? Headers = null,
        TimeSpan? Delay = null,
        string ContentType = "application/json"
    );

    private readonly TcpListener _listener = new(IPAddress.Loopback, 0);
    private readonly Func<Request, Reply> _respond;
    private readonly List<Request> _seen = [];
    private readonly CancellationTokenSource _stop = new();

    public RawServer(Func<Request, Reply> respond)
    {
        _respond = respond;
        _listener.Start();
        _ = Task.Run(AcceptAsync);
    }

    public string Url => $"http://127.0.0.1:{((IPEndPoint)_listener.LocalEndpoint).Port}";

    /// <summary>The requests that came in, in order.</summary>
    public IReadOnlyList<Request> Seen
    {
        get
        {
            lock (_seen)
            {
                return _seen.ToArray();
            }
        }
    }

    private async Task AcceptAsync()
    {
        try
        {
            while (!_stop.IsCancellationRequested)
            {
                var client = await _listener.AcceptTcpClientAsync(_stop.Token);
                _ = Task.Run(() => ServeAsync(client));
            }
        }
        catch (Exception e) when (e is OperationCanceledException or ObjectDisposedException or SocketException) { }
    }

    private async Task ServeAsync(TcpClient client)
    {
        using (client)
        {
            try
            {
                var stream = client.GetStream();
                var request = await ReadAsync(stream);
                if (request is null)
                {
                    return;
                }
                var reply = _respond(request);
                if (reply.Delay is { } delay)
                {
                    await Task.Delay(delay, _stop.Token);
                }
                var body = Encoding.UTF8.GetBytes(reply.Body);
                var head = new StringBuilder($"HTTP/1.1 {reply.Status} Status\r\n");
                head.Append($"Content-Type: {reply.ContentType}\r\nContent-Length: {body.Length}\r\nConnection: close\r\n");
                foreach (var (name, value) in reply.Headers ?? new Dictionary<string, string>())
                {
                    head.Append($"{name}: {value}\r\n");
                }
                head.Append("\r\n");
                await stream.WriteAsync(Encoding.ASCII.GetBytes(head.ToString()), _stop.Token);
                await stream.WriteAsync(body, _stop.Token);
                await stream.FlushAsync(_stop.Token);
            }
            catch (Exception e) when (e is IOException or SocketException or OperationCanceledException or ObjectDisposedException) { }
        }
    }

    private async Task<Request?> ReadAsync(NetworkStream stream)
    {
        var data = new MemoryStream();
        var chunk = new byte[4096];
        int end;
        while (true)
        {
            var text = Encoding.Latin1.GetString(data.GetBuffer(), 0, (int)data.Length);
            end = text.IndexOf("\r\n\r\n", StringComparison.Ordinal);
            if (end >= 0)
            {
                break;
            }
            var read = await stream.ReadAsync(chunk, _stop.Token);
            if (read == 0)
            {
                return null;
            }
            data.Write(chunk, 0, read);
        }
        var lines = Encoding.Latin1.GetString(data.GetBuffer(), 0, end).Split("\r\n");
        var headers = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        foreach (var line in lines.Skip(1))
        {
            var colon = line.IndexOf(':');
            var name = line[..colon].Trim();
            var value = line[(colon + 1)..].Trim();
            headers[name] = headers.TryGetValue(name, out var earlier) ? $"{earlier},{value}" : value;
        }
        var length = headers.TryGetValue("Content-Length", out var declared) ? int.Parse(declared, CultureInfo.InvariantCulture) : 0;
        while (data.Length < end + 4 + length)
        {
            var read = await stream.ReadAsync(chunk, _stop.Token);
            if (read == 0)
            {
                return null;
            }
            data.Write(chunk, 0, read);
        }
        var body = Encoding.UTF8.GetString(data.GetBuffer(), end + 4, length);
        lock (_seen)
        {
            var request = new Request(_seen.Count + 1, lines[0], headers, body);
            _seen.Add(request);
            return request;
        }
    }

    public void Dispose()
    {
        _stop.Cancel();
        _listener.Stop();
        _stop.Dispose();
    }
}

/// <summary>What the SDK puts on the wire and how it reads what comes back, over real sockets.</summary>
public class WireTests
{
    private const string ThingJson = """
        {"id":"t1","name":"n","created_at":"2024-01-02T03:04:05Z","kind":"alpha","count":1,
        "nullable_required":null,"tags":[],"metadata":{},"attrs":{}}
        """;

    private static Reply Ok(string body = ThingJson) => new(200, body);

    private static Reply Unavailable() =>
        new(503, """{"error":"unavailable"}""", new Dictionary<string, string> { ["retry-after"] = "0" });

    private static TortureClient Client(
        RawServer server,
        string prefix = "/v1",
        string? token = "token",
        TortureClientOptions? options = null
    )
    {
        options ??= new TortureClientOptions { RetrySchedule = [] };
        options.BaseUrl = server.Url + prefix;
        return new TortureClient(token, options);
    }

    private static string? Header(RawServer.Request request, string name) =>
        request.Headers.TryGetValue(name, out var value) ? value : null;

    [Theory]
    [InlineData("plain", "plain")]
    [InlineData("sp ace", "sp%20ace")]
    [InlineData("sl/ash", "sl%2Fash")]
    [InlineData("q?mark", "q%3Fmark")]
    [InlineData("per%cent", "per%25cent")]
    [InlineData("ha#sh", "ha%23sh")]
    [InlineData("lit%25eral", "lit%2525eral")]
    [InlineData("a+b", "a%2Bb")]
    [InlineData("héllo wörld ✓", "h%C3%A9llo%20w%C3%B6rld%20%E2%9C%93")]
    [InlineData("🎉", "%F0%9F%8E%89")]
    [InlineData("%2e%2e", "%252e%252e")]
    [InlineData("a/../b", "a%2F..%2Fb")]
    [InlineData("a;b,c=d&e", "a%3Bb%2Cc%3Dd%26e")]
    public async Task PathParametersAreEscapedOnTheWire(string value, string encoded)
    {
        using var server = new RawServer(_ => Ok());
        using var client = Client(server);
        await client.Things.RetrieveAsync(value);
        var request = Assert.Single(server.Seen);
        Assert.Equal($"GET /v1/things/{encoded} HTTP/1.1", request.Line);
    }

    [Theory]
    [InlineData(".")]
    [InlineData("..")]
    public async Task DotSegmentsAreRefusedBeforeAnythingIsSent(string value)
    {
        using var server = new RawServer(_ => Ok());
        using var client = Client(server);
        await Assert.ThrowsAnyAsync<TortureException>(() => client.Things.RetrieveAsync(value));
        await Assert.ThrowsAnyAsync<TortureException>(() => client.Things.UpdateAsync(value, new ThingPatch()));
        Assert.Empty(server.Seen);
    }

    [Fact]
    public async Task QueryValuesAreEscapedOnTheWire()
    {
        using var server = new RawServer(_ => Ok("""{"data":[]}"""));
        using var client = Client(server);
        await client.Things.ListAsync(
            new() { XRequired = "r", Ids = ["a&b", "c d", "é", "100%", "x=y", "a+b", "x#y", "q?"] }
        );
        var request = Assert.Single(server.Seen);
        Assert.Equal(
            "GET /v1/things?ids=a%26b&ids=c%20d&ids=%C3%A9&ids=100%25&ids=x%3Dy&ids=a%2Bb&ids=x%23y&ids=q%3F HTTP/1.1",
            request.Line
        );
    }

    [Fact]
    public async Task TheRequiredHeaderIsSentAndTheOptionalOneOnlyWhenGiven()
    {
        using var server = new RawServer(_ => Ok("""{"data":[]}"""));
        using var client = Client(server);
        await client.Things.ListAsync(new() { XRequired = "r1" });
        await client.Things.ListAsync(new() { XRequired = "r2", XTraceId = "t2" });
        Assert.Equal("r1", Header(server.Seen[0], "X-Required"));
        Assert.Null(Header(server.Seen[0], "X-Trace-Id"));
        Assert.Equal("r2", Header(server.Seen[1], "X-Required"));
        Assert.Equal("t2", Header(server.Seen[1], "X-Trace-Id"));
    }

    [Theory]
    [InlineData("/api/v2")]
    [InlineData("/api/v2/")]
    public async Task TheBaseUrlPathPrefixIsKept(string prefix)
    {
        using var server = new RawServer(request =>
            request.Line.Contains("/things/", StringComparison.Ordinal) ? Ok() : Ok("""{"data":[]}""")
        );
        using var client = Client(server, prefix);
        await client.Things.RetrieveAsync("t1");
        await client.Things.ListAsync(new() { XRequired = "r" });
        Assert.Equal("GET /api/v2/things/t1 HTTP/1.1", server.Seen[0].Line);
        Assert.Equal("GET /api/v2/things HTTP/1.1", server.Seen[1].Line);
    }

    [Fact]
    public async Task AnUnauthenticatedCallSendsNoCredentialsAndRaisesTheAuthError()
    {
        using var server = new RawServer(_ => new(401, """{"error":"unauthorized"}"""));
        using var anonymous = Client(server, token: null);
        var error = await Assert.ThrowsAsync<UnauthorizedException>(() => anonymous.Things.RetrieveAsync("t1"));
        Assert.Null(Header(server.Seen[0], "Authorization"));
        Assert.Equal(HttpStatusCode.Unauthorized, error.StatusCode);
        Assert.Equal("unauthorized", Assert.IsType<JsonElement>(error.Error).GetProperty("error").GetString());
        Assert.IsAssignableFrom<TortureException>(error);

        using var authenticated = Client(server, token: "tok");
        await Assert.ThrowsAsync<UnauthorizedException>(() => authenticated.Things.RetrieveAsync("t1"));
        Assert.Equal("Bearer tok", Header(server.Seen[1], "Authorization"));
    }

    [Theory]
    [InlineData(400, typeof(BadRequestException))]
    [InlineData(401, typeof(UnauthorizedException))]
    [InlineData(403, typeof(ForbiddenException))]
    [InlineData(404, typeof(NotFoundException))]
    [InlineData(409, typeof(ConflictException))]
    [InlineData(422, typeof(UnprocessableEntityException))]
    [InlineData(429, typeof(RateLimitException))]
    [InlineData(500, typeof(ServerErrorException))]
    [InlineData(503, typeof(ServerErrorException))]
    [InlineData(418, typeof(ApiException))]
    public async Task ErrorStatusesAreThrownAsTheirClassWithTheRequestId(int status, Type kind)
    {
        using var server = new RawServer(_ =>
            new(status, """{"message":"m"}""", new Dictionary<string, string> { ["x-request-id"] = "req_9" })
        );
        using var client = Client(server);
        var error = await Assert.ThrowsAnyAsync<ApiException>(() => client.Things.RetrieveAsync("t1"));
        Assert.Equal(kind, error.GetType());
        Assert.Equal(status, (int)error.StatusCode);
        Assert.Equal("req_9", error.RequestId);
        Assert.Equal("""{"message":"m"}""", error.Body);
        Assert.Single(server.Seen);
    }

    [Fact]
    public async Task TheRequestIdComesFromEitherHeader()
    {
        using var server = new RawServer(request =>
            request.Number switch
            {
                1 => new Reply(500, "{}", new Dictionary<string, string> { ["request-id"] = "alt" }),
                2 => new Reply(
                    500,
                    "{}",
                    new Dictionary<string, string> { ["request-id"] = "alt", ["x-request-id"] = "main" }
                ),
                _ => new Reply(500),
            }
        );
        using var client = Client(server);
        var ids = new List<string?>();
        for (var i = 0; i < 3; i++)
        {
            var error = await Assert.ThrowsAsync<ServerErrorException>(() => client.Things.RetrieveAsync("t1"));
            ids.Add(error.RequestId);
        }
        Assert.Equal(["alt", "main", null], ids);
    }

    [Fact]
    public async Task ErrorBodiesThatAreNotJsonOrEmptyAreKeptAsText()
    {
        using var server = new RawServer(request =>
            request.Number switch
            {
                1 => new Reply(502, "<html>Bad Gateway</html>", ContentType: "text/html"),
                2 => new Reply(500, ""),
                3 => new Reply(404, """{"unexpected":[1]}"""),
                _ => new Reply(422, """{"message":5}"""),
            }
        );
        using var client = Client(server);

        var html = await Assert.ThrowsAsync<ServerErrorException>(() => client.Things.RetrieveAsync("t1"));
        Assert.Equal("<html>Bad Gateway</html>", html.Body);
        Assert.Null(html.Error);
        Assert.Null(html.GetError<Problem>());
        Assert.Contains("502", html.Message);
        Assert.Null(html.RequestId);

        var empty = await Assert.ThrowsAsync<ServerErrorException>(() => client.Things.RetrieveAsync("t1"));
        Assert.Equal("", empty.Body);
        Assert.Null(empty.Error);
        Assert.Null(empty.GetError<Problem>());

        var other = await Assert.ThrowsAsync<NotFoundException>(() => client.Things.RetrieveAsync("t1"));
        var element = Assert.IsType<JsonElement>(other.Error);
        Assert.Equal(1, element.GetProperty("unexpected").GetArrayLength());
        Assert.Null(other.GetError<Problem>());

        // The declared schema of a 422 does not fit this body: it stays JSON.
        var mismatch = await Assert.ThrowsAsync<UnprocessableEntityException>(
            () => client.Things.CreateAsync(new ThingCreate { Name = "n", Kind = Kind.Alpha })
        );
        Assert.Equal(5, Assert.IsType<JsonElement>(mismatch.Error).GetProperty("message").GetInt32());
        Assert.Null(mismatch.GetError<ValidationError>());
    }

    [Fact]
    public async Task UnknownFieldsOfAResponseAreTolerated()
    {
        using var server = new RawServer(request =>
            request.Line.StartsWith("POST", StringComparison.Ordinal)
                ? new Reply(200, """{"type":"circle","radius":2,"extra":true}""")
                : Ok(
                    """
                    {"id":"t1","name":"n","created_at":"2024-01-02T03:04:05Z","kind":"alpha","count":1,
                    "nullable_required":null,"tags":[],"metadata":{},"attrs":{},
                    "nested_map":{"a":[{"line1":"l","zip":"z"}]},"brand_new":{"deep":[1,{"x":null}]}}
                    """
                )
        );
        using var client = Client(server);
        var thing = await client.Things.RetrieveAsync("t1");
        Assert.Equal("t1", thing.Id);
        Assert.Equal(1, thing.AdditionalProperties!["brand_new"].GetProperty("deep")[0].GetInt32());
        Assert.Equal("z", thing.NestedMap!["a"][0].AdditionalProperties!["zip"].GetString());

        var shape = await client.Shapes.CreateAsync(new Shape.Circle(new Circle { Radius = 1 }));
        Assert.Equal(2, Assert.IsType<Shape.Circle>(shape).Value.Radius);
    }

    [Fact]
    public async Task ANullBodyOfANullableResponseIsNull()
    {
        using var server = new RawServer(request =>
            request.Number == 1 ? new Reply(200, "null") : new Reply(200, """{"id":"j1"}""")
        );
        using var client = Client(server);
        Assert.Null(await client.Jobs.RetrieveAsync("j1"));
        Assert.Equal("j1", (await client.Jobs.RetrieveAsync("j1"))?.Id);
    }

    [Fact]
    public async Task TheIdempotencyKeyAndRequestIdAreTheSameOnEveryAttempt()
    {
        using var server = new RawServer(request =>
            request.Number == 1 ? Unavailable() : new Reply(201, ThingJson)
        );
        var options = new TortureClientOptions { RetrySchedule = [TimeSpan.Zero, TimeSpan.Zero] };
        using var client = Client(server, options: options);
        var body = new ThingCreate { Name = "n", Kind = Kind.Alpha };
        await client.Things.CreateAsync(body);
        Assert.Equal(2, server.Seen.Count);
        var generated = Header(server.Seen[0], "Idempotency-Key");
        Assert.StartsWith("auto_", generated);
        Assert.Equal(generated, Header(server.Seen[1], "Idempotency-Key"));
        Assert.Equal(server.Seen[0].Body, server.Seen[1].Body);
        Assert.Equal("""{"kind":"alpha","name":"n"}""", server.Seen[0].Body);
        Assert.NotNull(Header(server.Seen[0], "torture-req-id"));
        Assert.Equal(Header(server.Seen[0], "torture-req-id"), Header(server.Seen[1], "torture-req-id"));
        Assert.Null(Header(server.Seen[0], "torture-retry-count"));
        Assert.Equal("1", Header(server.Seen[1], "torture-retry-count"));

        // A key the caller gives is kept on every attempt too, and the next call gets a new one.
        await client.Things.CreateAsync(body, requestOptions: new() { IdempotencyKey = "mine" });
        await client.Things.CreateAsync(body);
        Assert.Equal("mine", Header(server.Seen[2], "Idempotency-Key"));
        Assert.NotEqual(generated, Header(server.Seen[3], "Idempotency-Key"));
    }

    [Fact]
    public async Task ATimedOutAttemptIsRetried()
    {
        using var server = new RawServer(request =>
            request.Number == 1 ? new Reply(200, ThingJson, Delay: TimeSpan.FromSeconds(30)) : Ok()
        );
        var options = new TortureClientOptions
        {
            RetrySchedule = [TimeSpan.Zero],
            Timeout = TimeSpan.FromMilliseconds(300),
        };
        using var client = Client(server, options: options);
        var thing = await client.Things.RetrieveAsync("t1");
        Assert.Equal("t1", thing.Id);
        Assert.Equal(2, server.Seen.Count);
    }

    [Fact]
    public async Task EveryAttemptTimingOutRaisesTheTimeoutError()
    {
        using var server = new RawServer(_ => new Reply(200, ThingJson, Delay: TimeSpan.FromSeconds(30)));
        var options = new TortureClientOptions
        {
            RetrySchedule = [TimeSpan.Zero],
            Timeout = TimeSpan.FromMilliseconds(200),
        };
        using var client = Client(server, options: options);
        var error = await Assert.ThrowsAsync<ApiTimeoutException>(() => client.Things.RetrieveAsync("t1"));
        Assert.IsAssignableFrom<ApiConnectionException>(error);
        Assert.IsAssignableFrom<TortureException>(error);
        Assert.Equal(2, server.Seen.Count);
    }

    [Fact]
    public async Task CancellingWhileWaitingToRetryStopsTheCall()
    {
        using var server = new RawServer(_ =>
            new(429, "{}", new Dictionary<string, string> { ["retry-after"] = "30" })
        );
        var options = new TortureClientOptions { MaxRetries = 2 };
        using var client = Client(server, options: options);
        using var cancel = new CancellationTokenSource(TimeSpan.FromMilliseconds(500));
        var clock = Stopwatch.StartNew();
        await Assert.ThrowsAnyAsync<OperationCanceledException>(
            () => client.Things.RetrieveAsync("t1", cancellationToken: cancel.Token)
        );
        Assert.InRange(clock.Elapsed, TimeSpan.Zero, TimeSpan.FromSeconds(20));
        Assert.Single(server.Seen);
    }

    [Fact]
    public async Task CancellingWhileAnAttemptIsInFlightIsNotATimeout()
    {
        using var server = new RawServer(_ => new Reply(200, ThingJson, Delay: TimeSpan.FromSeconds(30)));
        using var client = Client(server);
        using var cancel = new CancellationTokenSource(TimeSpan.FromMilliseconds(300));
        var clock = Stopwatch.StartNew();
        var error = await Assert.ThrowsAnyAsync<OperationCanceledException>(
            () => client.Things.RetrieveAsync("t1", cancellationToken: cancel.Token)
        );
        Assert.IsNotType<ApiTimeoutException>(error);
        Assert.InRange(clock.Elapsed, TimeSpan.Zero, TimeSpan.FromSeconds(20));
    }

    [Fact]
    public async Task RetryAfterAsAnHttpDateIsWaitedFor()
    {
        var date = DateTimeOffset.UtcNow.AddSeconds(2).ToString("r", CultureInfo.InvariantCulture);
        using var server = new RawServer(request =>
            request.Number == 1
                ? new Reply(429, "{}", new Dictionary<string, string> { ["retry-after"] = date })
                : Ok()
        );
        var options = new TortureClientOptions { MaxRetries = 2 };
        using var client = Client(server, options: options);
        var clock = Stopwatch.StartNew();
        await client.Things.RetrieveAsync("t1");
        Assert.InRange(clock.Elapsed, TimeSpan.FromSeconds(0.5), TimeSpan.FromSeconds(10));
        Assert.Equal(2, server.Seen.Count);
    }

    private sealed class Logging(string name, List<string> log) : DelegatingHandler
    {
        protected override Task<HttpResponseMessage> SendAsync(
            HttpRequestMessage request,
            CancellationToken cancellationToken
        )
        {
            var attempt = request.Headers.TryGetValues("torture-retry-count", out var count) ? count.Single() : "0";
            log.Add($"{name}:{attempt}");
            return base.SendAsync(request, cancellationToken);
        }
    }

    [Fact]
    public async Task MiddlewareRunsOnceForEveryAttemptInOrder()
    {
        using var server = new RawServer(request => request.Number == 1 ? Unavailable() : Ok());
        var log = new List<string>();
        var options = new TortureClientOptions { RetrySchedule = [TimeSpan.Zero] };
        options.Handlers.Add(new Logging("outer", log));
        options.Handlers.Add(new Logging("inner", log));
        using var client = Client(server, options: options);
        await client.Things.RetrieveAsync("t1");
        Assert.Equal(["outer:0", "inner:0", "outer:1", "inner:1"], log);
        Assert.Equal(2, server.Seen.Count);
    }
}

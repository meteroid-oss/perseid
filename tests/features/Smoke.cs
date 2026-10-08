using System.Diagnostics;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using Features;
using Features.Models;

var serverUrl = Environment.GetEnvironmentVariable("FEATURES_URL")!;

async Task<List<string>> Collect<T>(IAsyncEnumerable<T> items, Func<T, string> id)
{
    var ids = new List<string>();
    await foreach (var item in items)
    {
        ids.Add(id(item));
    }
    return ids;
}

void Equal<T>(T expected, T actual)
{
    if (!EqualityComparer<T>.Default.Equals(expected, actual))
    {
        throw new Exception($"expected {expected}, got {actual}");
    }
}

void Ids(string expected, List<string> actual) => Equal(expected, string.Join(",", actual));

async Task Cancelled(Func<Task> call)
{
    try
    {
        await call();
    }
    catch (OperationCanceledException)
    {
        return;
    }
    throw new Exception("expected the call to be cancelled");
}

using var client = new FeaturesClient("tok", new() { BaseUrl = serverUrl });
Equal("||", (await client.Account.CheckHealthAsync()).Status);
Equal("Bearer tok||", (await client.Account.RetrieveMachineAsync()).Status);
Ids("w1,w2,w3", await Collect(client.Widgets.ListAsync(), w => w.Id));
Ids(
    "e1,e2,e3",
    await Collect(client.Widgets.ListEventsAsync("w1", new() { Kind = "created" }), e => e.Id)
);
Ids(
    "e7,e8,e6",
    await Collect(
        client.Widgets.ListEventsAsync("w1", new() { Kind = "created", EndingBefore = "e9" }),
        e => e.Id
    )
);
Ids("g1,g2,g3", await Collect(client.Gadgets.ListAsync(), g => g.Id));
Ids("r1,r2,r3", await Collect(client.Records.ListAsync(), r => r.Id));

var pages = new List<string>();
await foreach (var page in client.Widgets.ListAsync().AsPagesAsync())
{
    pages.Add($"{string.Join("+", page.Items.Select(w => w.Id))}:{page.HasNextPage}:{page.NextCursor}");
}
Equal("w1+w2:True:c2,w3:False:", string.Join(",", pages));
var widgets = await client.Widgets.ListAsync();
Equal("w1", widgets.Data[0].Id);
Equal("c2", widgets.NextCursor);
Equal(widgets.Body.NextCursor, widgets.NextCursor);
Ids("w1,w2,w3", await Collect(widgets, w => w.Id));
var gadgets = await client.Gadgets.ListAsync();
Equal(2, gadgets.Meta.TotalPages);
Equal(0, gadgets.Meta.Page);
var nextGadgets = await gadgets.GetNextPageAsync();
Equal(1, nextGadgets.Meta.Page);
Equal("g3", nextGadgets.Items.Single().Id);
Equal(false, nextGadgets.HasNextPage);
var records = await client.Records.ListAsync().ConfigureAwait(false);
Equal(3L, records.Total);
Equal("r1", records.Data[0].Id);
Ids("r1,r2,r3", await Collect(records, r => r.Id));
using (var cancelled = new CancellationTokenSource())
{
    cancelled.Cancel();
    await Cancelled(async () => await client.Records.ListAsync(cancellationToken: cancelled.Token));
    await Cancelled(async () =>
    {
        await foreach (var record in client.Records.ListAsync().WithCancellation(cancelled.Token))
        {
            throw new Exception($"no record once cancelled, got {record.Id}");
        }
    });
    await Cancelled(async () => await Collect(records.AsPagesAsync(cancelled.Token), p => p.Total.ToString()));
}

var widget = (await client.Widgets.ListAsync()).Data[0];
Equal("red", widget.AdditionalProperties!["color"].GetString());
var widgetJson = JsonSerializer.Serialize(widget, FeaturesJsonContext.Default.Widget);
Equal("""{"id":"w1","name":"w1","color":"red"}""", widgetJson);
Equal(widget, JsonSerializer.Deserialize(widgetJson, FeaturesJsonContext.Default.Widget));

var raw = await client.Widgets.WithRawResponse.ListAsync();
Equal("req_mock", raw.RequestId);
Equal(200, (int)raw.StatusCode);
Equal("w1", raw.Value.Data[0].Id);

using var basic = new FeaturesClient(
    null,
    new() { BaseUrl = serverUrl, BasicAuth = new("u", "p") }
);
Equal("Basic dTpw||", (await basic.Account.CreateSessionAsync()).Status);

using var provided = new FeaturesClient(
    null,
    new() { BaseUrl = serverUrl, TokenProvider = _ => ValueTask.FromResult("fresh") }
);
Equal("Bearer fresh||", (await provided.Account.RetrieveMachineAsync()).Status);

using var keyed = new FeaturesClient(
    null,
    new() { BaseUrl = serverUrl, ApiKeys = new() { ApiKey = "k" } }
);
Ids("w1,w2,w3", await Collect(keyed.Widgets.ListAsync(), w => w.Id));
using var anonymous = new FeaturesClient(null, new() { BaseUrl = serverUrl });
try
{
    await anonymous.Widgets.ListAsync();
    throw new Exception("an unauthenticated call must fail");
}
catch (UnauthorizedException e)
{
    Equal("req_mock", e.RequestId);
    Equal("unauthorized", ((JsonElement)e.Error!).GetProperty("error").GetString());
}

Environment.SetEnvironmentVariable("FEATURES_API_KEY", "envtok");
Environment.SetEnvironmentVariable("FEATURES_BASE_URL", serverUrl);
using (var fromEnvironment = new FeaturesClient())
{
    Equal("Bearer envtok||", (await fromEnvironment.Account.RetrieveMachineAsync()).Status);
}
using (var explicitly = new FeaturesClient("tok", new() { BaseUrl = "http://127.0.0.1:9" }))
{
    try
    {
        await explicitly.Account.CheckHealthAsync(new() { MaxRetries = 0 });
        throw new Exception("the explicit base URL must win");
    }
    catch (ApiConnectionException e) when (e is not ApiTimeoutException) { }
}
Environment.SetEnvironmentVariable("FEATURES_API_KEY", null);
Environment.SetEnvironmentVariable("FEATURES_BASE_URL", null);

var events = new List<SseEvent>();
await using (var stream = await client.Streaming.RetrieveEventsStreamAsync(new() { Topic = "news" }))
{
    await foreach (var sse in stream)
    {
        events.Add(sse);
    }
    Equal("3", stream.LastEventId);
}
Equal(3, events.Count);
Equal(new SseEvent("greeting", "news", "1"), events[0]);
Equal(new SseEvent("message", "line1\nline2", "1"), events[1]);
Equal(new SseEvent("message", "{\"n\": 3}", "3", 1500), events[2]);

var completion = new CompletionRequest { Prompt = "hi" };
Equal("HI", (await client.Streaming.CreateCompletionAsync(completion)).Text);
var deltas = new List<string>();
await using (var chunks = await client.Streaming.CreateCompletionStreamAsync(completion))
{
    await foreach (var chunk in chunks)
    {
        deltas.Add(chunk.Delta);
        Equal("message", chunks.LastEvent!.Event);
    }
}
Equal("h,i", string.Join(",", deltas));
Equal(null, completion.Stream);

var uploaded = await client.Streaming.UploadFileAsync(
    new()
    {
        File = Upload.FromBytes(Encoding.UTF8.GetBytes("hello"), "a.txt", "text/plain"),
        Name = "doc",
        Count = 2,
        Meta = new Health { Status = "ok" },
        Tags = ["a", "b"],
    }
);
Equal(
    "count=::2;file=a.txt:text/plain:hello;meta=:application/json:{\"status\":\"ok\"};name=::doc;tags=::a;tags=::b",
    uploaded.Status
);
Equal(
    "application/octet-stream:raw bytes",
    (await client.Streaming.UploadContentAsync("f1", Encoding.UTF8.GetBytes("raw bytes"))).Status
);
using var streamed = new MemoryStream(Encoding.UTF8.GetBytes("streamed"));
Equal(
    "application/octet-stream:streamed",
    (await client.Streaming.UploadContentAsync("f1", streamed)).Status
);

var searched = await client.Wire.SearchAsync(
    new()
    {
        Filter = new Filter { Status = "open", Amount = new FilterAmount { Gte = 5 } },
        Expand = ["a", "b"],
        Metadata = new() { ["k"] = "v" },
        Ids = new List<string> { "x", "y" },
        Tags = ["t1", "t2"],
        Range = new SearchRange { Gte = 1, Lt = 9 },
        Created = new RangeQuerySpecs { Gte = 3, Lt = 7 },
    }
);
Equal(
    "created[gte]=3&created[lt]=7&expand[]=a&expand[]=b&filter[amount][gte]=5&filter[status]=open&ids=x&ids=y"
        + "&metadata[k]=v&range[gte]=1&range[lt]=9&tags=t1,t2",
    searched.Status
);
Equal("created=5", (await client.Wire.SearchAsync(new() { Created = 5 })).Status);
var charged = await client.Wire.CreateChargeAsync(
    new Charge
    {
        Amount = 100,
        Capture = true,
        Metadata = new Dictionary<string, string> { ["order"] = "7" },
        Items = [new ChargeItemsItem { Price = "p1", Quantity = 2 }, new ChargeItemsItem { Price = "p2" }],
        Expand = ["customer"],
        Statuses = ["a", "b"],
        Codes = ["c1", "c2"],
        Shipping = new ChargeShipping
        {
            Address = new ChargeShippingAddress { Line1 = "1 Main", City = "Paris" },
        },
    }
);
Equal(
    "application/x-www-form-urlencoded|amount=100&capture=true&codes=c1,c2&expand[]=customer"
        + "&items[0][price]=p1&items[0][quantity]=2&items[1][price]=p2&metadata[order]=7"
        + "&shipping[address][city]=Paris&shipping[address][line1]=1 Main&statuses=a&statuses=b",
    charged.Status
);
Equal(
    "beta=true&limit=2|features=x,y",
    (await client.Wire.BetaSearchAsync(new() { Limit = 2, Features = ["x", "y"] })).Status
);
Equal(
    "42:image/png:png",
    (await client.Wire.UpdateImageAsync("42", Encoding.UTF8.GetBytes("png"))).Status
);
await Scenarios.RunAsync(serverUrl);
Console.WriteLine("csharp smoke test passed");

/// <summary>The scenarios of tests/features/SCENARIOS.md, against the strict mock server.</summary>
internal static class Scenarios
{
    private static readonly HttpClient s_http = new();
    private static int s_count;

    public static async Task RunAsync(string url)
    {
        using var client = Make(url);
        await ItemsAsync(client);
        await ContentAsync(client);
        await EncodingAsync(client, url);
        await CookiesAsync(client, url);
        await OAuthAsync(url);
        await RetriesAsync(client, url);
        await ErrorsAsync(url);
        await StreamingAsync(client, url);
        await CancellationAsync(client, url);
    }

    /// <summary>A client of the mock server with the token <c>tok</c>.</summary>
    private static FeaturesClient Make(string url, Capture? capture = null)
    {
        var options = new FeaturesClientOptions { BaseUrl = url };
        if (capture is not null)
        {
            options.Handlers.Add(capture);
        }
        return new FeaturesClient("tok", options);
    }

    /// <summary>An <c>X-Scenario-Id</c> unique to one test case.</summary>
    private static string ScenarioId(string name) =>
        $"csharp-{name}-{Interlocked.Increment(ref s_count)}";

    /// <summary>What the mock server saw for a scenario id: its attempts and idempotency keys.</summary>
    private static async Task<(int Attempts, string[] Keys)> State(string url, string id)
    {
        using var reply = await s_http.GetAsync($"{url}/__server/attempts/{Uri.EscapeDataString(id)}");
        using var document = JsonDocument.Parse(await reply.Content.ReadAsStringAsync());
        var root = document.RootElement;
        return (
            root.GetProperty("attempts").GetInt32(),
            root.GetProperty("keys").EnumerateArray().Select(key => key.GetString()!).ToArray()
        );
    }

    private static string Show(object? value) => value?.ToString() ?? "null";

    private static void Check(bool condition, string what)
    {
        if (!condition)
        {
            throw new Exception($"failed: {what}");
        }
    }

    private static void Equal<T>(T expected, T actual)
    {
        if (!EqualityComparer<T>.Default.Equals(expected, actual))
        {
            throw new Exception($"expected {Show(expected)}, got {Show(actual)}");
        }
    }

    private static void Same<T>(IEnumerable<T> expected, IEnumerable<T> actual)
    {
        var want = expected.ToList();
        var got = actual.ToList();
        if (!want.SequenceEqual(got))
        {
            throw new Exception(
                $"expected [{string.Join(", ", want.Select(x => Show(x)))}], got [{string.Join(", ", got.Select(x => Show(x)))}]"
            );
        }
    }

    private static void IsNull(object? value, string what)
    {
        if (value is not null)
        {
            throw new Exception($"{what}: expected null, got {Show(value)}");
        }
    }

    /// <summary>The exception <paramref name="call"/> throws, which must be a <typeparamref name="T"/>.</summary>
    private static async Task<T> Throws<T>(Func<Task> call)
        where T : Exception
    {
        try
        {
            await call();
        }
        catch (T e)
        {
            return e;
        }
        throw new Exception($"expected {typeof(T).Name}, but the call succeeded");
    }

    /// <summary>The model of an error body, which the operation declares.</summary>
    private static Error Declared(ApiException error) =>
        error.Error as Error ?? throw new Exception($"the error body is not an Error: {error.Body}");

    /// <summary>Keeps the headers of every HTTP attempt that goes through the client.</summary>
    private sealed class Capture : DelegatingHandler
    {
        public List<Dictionary<string, string>> Headers { get; } = [];

        protected override Task<HttpResponseMessage> SendAsync(
            HttpRequestMessage request,
            CancellationToken cancellationToken
        )
        {
            var seen = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
            foreach (var header in request.Headers)
            {
                seen[header.Key] = string.Join(",", header.Value);
            }
            Headers.Add(seen);
            return base.SendAsync(request, cancellationToken);
        }
    }

    private static async Task ItemsAsync(FeaturesClient client)
    {
        var item = await client.Items.RetrieveAsync("i1");
        Equal("i1", item.Id);
        Equal("first", item.Name);
        Equal("hi", item.Note);

        // The unset note is omitted from the body: the server requires exactly {"name":"renamed"}.
        var patched = await client.Items.UpdateAsync("i1", new ItemPatch { Name = "renamed" });
        Equal("renamed", patched.Name);
        IsNull(patched.Note, "the note of the patched item");

        // 204 without a body or a content type.
        await client.Items.DeleteAsync("i1");
        var raw = await client.Items.WithRawResponse.DeleteAsync("i1");
        Equal(204, (int)raw.StatusCode);
    }

    private static async Task ContentAsync(FeaturesClient client)
    {
        var content = client.Content;
        IsNull(await content.RetrieveScenariosNullableBodyAsync(), "a null body");
        Equal("hello text\n", await content.RetrieveScenariosTextAsync());
        Equal("id,name\n1,alpha\n2,\"be,ta\"\n", await content.RetrieveScenariosCsvAsync());

        var blob = await content.DownloadBlobAsync();
        Equal(256, blob.Length);
        Same(Enumerable.Range(0, 256).Select(i => (byte)i), blob);
        var png = new byte[] { 0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A }
            .Concat(Enumerable.Repeat(new byte[] { 0x00, 0x01, 0xFE, 0xFF }, 4).SelectMany(part => part))
            .ToArray();
        var image = await content.DownloadImageAsync();
        Equal(24, image.Length);
        Same(png, image);

        var malformed = await Throws<ApiDecodeException>(() => content.RetrieveScenariosMalformedAsync());
        Check(malformed.InnerException is JsonException, "a malformed body keeps the JSON error");
        Exception asException = malformed;
        Check(asException is FeaturesException and not ApiException, "a decode error is no API error");
        await Throws<ApiDecodeException>(() => content.RetrieveScenariosEmptyBodyAsync());

        var extras = await content.ListScenariosExtraFieldsAsync();
        Equal("ok", extras.Status);
        var unknown = extras.AdditionalProperties!;
        Equal(3, unknown.Count);
        Equal(1, unknown["extra"].GetInt32());
        Check(
            JsonNode.DeepEquals(
                JsonNode.Parse(unknown["nested"].GetRawText()),
                JsonNode.Parse("""{"a":[1,2,{"b":null}]}""")
            ),
            "the nested unknown field is kept"
        );
        Check(
            JsonNode.DeepEquals(JsonNode.Parse(unknown["list"].GetRawText()), JsonNode.Parse("""[1,"x"]""")),
            "the list unknown field is kept"
        );

        var nulls = await content.Nulls.RetrieveAsync();
        IsNull(nulls.Name, "name");
        Same<string?>(new string?[] { "a", null, "b" }, nulls.Tags);
        Equal(2, nulls.Counts.Count);
        Equal<long?>(1, nulls.Counts["x"]);
        IsNull(nulls.Counts["y"], "counts.y");
        IsNull(nulls.Note, "note");

        // The name is an explicit null and is sent; the note is unset and omitted.
        var sent = new NullBag
        {
            Name = null,
            Tags = new List<string?> { "a", null },
            Counts = new Dictionary<string, long?> { ["x"] = null, ["y"] = 2 },
        };
        var wire = JsonSerializer.Serialize(sent, FeaturesJsonContext.Default.NullBag);
        Check(
            JsonNode.DeepEquals(
                JsonNode.Parse(wire),
                JsonNode.Parse("""{"name":null,"tags":["a",null],"counts":{"x":null,"y":2}}""")
            ),
            $"the null bag is sent as {wire}"
        );
        var echoed = await content.Nulls.CreateAsync(sent);
        IsNull(echoed.Name, "echoed name");
        Same<string?>(new string?[] { "a", null }, echoed.Tags);
        IsNull(echoed.Counts["x"], "echoed counts.x");
        Equal<long?>(2, echoed.Counts["y"]);
        IsNull(echoed.Note, "echoed note");

        var bag = await content.RetrieveScenariosBagAsync();
        Equal("b1", bag.Id);
        var extra = bag.AdditionalProperties!;
        Equal(2, extra.Count);
        Equal(1, extra["a"].GetInt32());
        Equal(2, extra["b"].GetInt32());

        var labels = await content.ListScenariosLabelsAsync();
        Equal(2, labels.Count);
        Equal("v", labels["k"]);
        Equal("y", labels["z"]);

        var known = await content.RetrieveScenariosEnumAsync(new() { Mode = "known" });
        Equal("red", known.Kind.Value);
        Check(known.Kind.IsKnown, "red is a known kind");
        Same(new[] { "green", "blue" }, known.Kinds!.Select(kind => kind.Value));
        var other = await content.RetrieveScenariosEnumAsync(new() { Mode = "unknown" });
        Equal("magenta", other.Kind.Value);
        Check(!other.Kind.IsKnown, "magenta is not a known kind");
        var kinds = other.Kinds!;
        Same(new[] { "red", "magenta" }, kinds.Select(kind => kind.Value));
        Check(kinds[0].IsKnown && !kinds[1].IsKnown, "the known kinds of a list stay known");
    }

    private static async Task EncodingAsync(FeaturesClient client, string url)
    {
        var encoding = client.EncodingApi;

        var big = await encoding.ScenariosBigintAsync(
            new BigBox { Value = 9007199254740993L, Min = long.MinValue }
        );
        Equal(9007199254740993L, big.Value);
        Equal(long.MinValue, big.Min);

        var hello = new byte[] { 0x68, 0x65, 0x6C, 0x6C, 0x6F, 0xFB, 0xFF, 0xFE };
        var sentBlob = new Blob { Data = Convert.ToBase64String(hello) };
        Equal("aGVsbG/7//4=", sentBlob.Data);
        Same(hello, Convert.FromBase64String((await encoding.Bytes.CreateAsync(sentBlob)).Data));
        Same(hello, Convert.FromBase64String((await encoding.Bytes.RetrieveAsync()).Data));

        var instant = new DateTimeOffset(2024, 1, 2, 3, 4, 5, 250, TimeSpan.Zero);
        var spellings = new[]
        {
            instant,
            instant.ToOffset(TimeSpan.FromHours(2)),
            instant.ToOffset(new TimeSpan(-5, -30, 0)),
        };
        foreach (var at in spellings)
        {
            var queried = await encoding.RetrieveScenariosDatetimeAsync(new() { Since = at, Day = new DateOnly(2024, 1, 2) });
            Equal(instant, queried.At);
            Equal(new DateOnly(2024, 1, 2), queried.Day);
            var posted = await encoding.ScenariosDatetimeAsync(new DateBox { At = at, Day = new DateOnly(2024, 1, 2) });
            Equal(instant, posted.At);
            Equal(new DateOnly(2024, 1, 2), posted.Day);
        }

        foreach (
            var value in new[]
            {
                "plain",
                "sp ace",
                "sl/ash",
                "q?mark",
                "per%cent",
                "ha#sh",
                "lit%25eral",
                "a+b",
                "h\u00e9llo w\u00f6rld \u2713",
            }
        )
        {
            Equal(value, (await encoding.RetrieveScenarioPathAsync(value)).Status);
        }
        foreach (
            var value in new[] { "plain", "sp ace", "a&b=c+d", "100%", "slash/qm?", "h\u00e9llo w\u00f6rld \u2713" }
        )
        {
            Equal(value, (await encoding.RetrieveScenariosQueryAsync(new() { Q = value })).Status);
        }
        Equal(
            "b,a,c",
            (await encoding.RetrieveScenariosMultiAsync(new() { Ids = ["b", "a", "c"], Flag = true })).Status
        );

        // The first call carries no trace header, and neither call carries credentials.
        var capture = new Capture();
        using var traced = Make(url, capture);
        Equal("acme|", (await traced.EncodingApi.ListScenariosHeadersAsync(new() { XTenant = "acme" })).Status);
        Equal(
            "acme|t1",
            (await traced.EncodingApi.ListScenariosHeadersAsync(new() { XTenant = "acme", XTraceId = "t1" })).Status
        );
        Equal(2, capture.Headers.Count);
        Equal("acme", capture.Headers[0]["X-Tenant"]);
        Check(!capture.Headers[0].ContainsKey("X-Trace-Id"), "the unset trace header is not sent");
        Equal("t1", capture.Headers[1]["X-Trace-Id"]);
        Check(!capture.Headers[0].ContainsKey("Authorization"), "no credentials without security");
    }

    private static async Task CookiesAsync(FeaturesClient client, string url)
    {
        Equal("abc123", (await client.Cookies.RetrieveScenariosCookieAsync(new() { SessionId = "abc123" })).Status);

        // The API key cookie replaces the token of the client.
        var capture = new Capture();
        var options = new FeaturesClientOptions { BaseUrl = url, ApiKeys = new() { ApiKeyCookie = "ck1" } };
        options.Handlers.Add(capture);
        using var keyed = new FeaturesClient(null, options);
        Equal("ck1", (await keyed.Cookies.RetrieveScenariosCookieAuthAsync()).Status);
        Equal("auth_token=ck1", capture.Headers[0]["Cookie"]);
        Check(!capture.Headers[0].ContainsKey("Authorization"), "the cookie key is no bearer token");

        using var anonymous = new FeaturesClient(null, new() { BaseUrl = url });
        await Throws<UnauthorizedException>(() => anonymous.Cookies.RetrieveScenariosCookieAuthAsync());
    }

    private static async Task OAuthAsync(string url)
    {
        const string secret = "p@ss word";
        FeaturesClient Oauth(string id, string? token = null, bool inBody = false, string clientSecret = secret) =>
            new(
                token,
                new FeaturesClientOptions
                {
                    BaseUrl = url,
                    ClientId = id,
                    ClientSecret = clientSecret,
                    OAuthClientAuthInBody = inBody,
                    MaxRetries = 0,
                }
            );

        using (var cached = Oauth("csharp-oauth"))
        {
            Equal("Bearer at-csharp-oauth-1||", (await cached.Account.RetrieveMachineAsync()).Status);
            Equal("Bearer at-csharp-oauth-1||", (await cached.Account.RetrieveMachineAsync()).Status);
        }
        Equal(1, (await State(url, "csharp-oauth")).Attempts);

        using (var inBody = Oauth("csharp-oauth-body", inBody: true))
        {
            Equal("Bearer at-csharp-oauth-body-1||", (await inBody.Account.RetrieveMachineAsync()).Status);
        }

        using (var revoked = Oauth("csharp-oauth-revoked"))
        {
            Equal("Bearer at-csharp-oauth-revoked-2||", (await revoked.Account.RetrieveMachineAsync()).Status);
        }
        Equal(2, (await State(url, "csharp-oauth-revoked")).Attempts);

        using (var concurrent = Oauth("csharp-oauth-concurrent"))
        {
            var both = await Task.WhenAll(
                concurrent.Account.RetrieveMachineAsync(),
                concurrent.Account.RetrieveMachineAsync()
            );
            Equal("Bearer at-csharp-oauth-concurrent-1||", both[0].Status);
            Equal("Bearer at-csharp-oauth-concurrent-1||", both[1].Status);
        }
        Equal(1, (await State(url, "csharp-oauth-concurrent")).Attempts);

        using (var invalid = Oauth("csharp-oauth-bad", clientSecret: "wrong"))
        {
            var rejected = await Throws<UnauthorizedException>(() => invalid.Account.RetrieveMachineAsync());
            Equal("invalid_client", ((JsonElement)rejected.Error!).GetProperty("error").GetString());
        }

        using var withToken = Oauth("csharp-oauth", token: "tok");
        Equal("Bearer tok||", (await withToken.Account.RetrieveMachineAsync()).Status);
    }

    private static async Task RetriesAsync(FeaturesClient client, string url)
    {
        // Retry-After in seconds.
        var flaky = ScenarioId("flaky");
        var clock = Stopwatch.StartNew();
        Equal("attempt=2", (await client.Retries.RetrieveScenariosFlakyAsync(new() { XScenarioId = flaky })).Status);
        Check(clock.Elapsed >= TimeSpan.FromSeconds(0.9), $"Retry-After: 1 is waited for, took {clock.Elapsed}");
        Check(clock.Elapsed < TimeSpan.FromSeconds(10), $"only Retry-After is waited for, took {clock.Elapsed}");
        Equal(2, (await State(url, flaky)).Attempts);

        // Without retries.
        var once = ScenarioId("flaky-once");
        var unavailable = await Throws<ServerErrorException>(
            () =>
                client.Retries.RetrieveScenariosFlakyAsync(
                    new() { XScenarioId = once },
                    new RequestOptions { MaxRetries = 0 }
                )
        );
        Equal(503, (int)unavailable.StatusCode);
        Equal("unavailable", ((JsonElement)unavailable.Error!).GetProperty("error").GetString());
        Equal("req_mock", unavailable.RequestId);
        Equal(1, (await State(url, once)).Attempts);

        // Retry-After as an HTTP date, one or two seconds from now.
        var limited = ScenarioId("rate-limited");
        clock.Restart();
        Equal(
            "attempt=2",
            (await client.Retries.RetrieveScenariosRateLimitedAsync(new() { XScenarioId = limited })).Status
        );
        Check(clock.Elapsed >= TimeSpan.FromSeconds(0.5), $"the HTTP date is waited for, took {clock.Elapsed}");
        Check(clock.Elapsed < TimeSpan.FromSeconds(10), $"only the HTTP date is waited for, took {clock.Elapsed}");
        Equal(2, (await State(url, limited)).Attempts);

        // Retries are exhausted.
        var down = ScenarioId("unavailable");
        var exhausted = await Throws<ServerErrorException>(
            () =>
                client.Retries.RetrieveScenariosUnavailableAsync(
                    new() { XScenarioId = down },
                    new RequestOptions { MaxRetries = 2 }
                )
        );
        Equal(503, (int)exhausted.StatusCode);
        Equal(3, (await State(url, down)).Attempts);

        // The idempotency key survives the retry.
        var idempotent = ScenarioId("idempotent");
        var created = await client.Retries.ScenariosIdempotentAsync(
            new Payment { Amount = 5 },
            new() { XScenarioId = idempotent, IdempotencyKey = "idem-1" },
            new RequestOptions { MaxRetries = 2 }
        );
        Equal("attempts=2;key=idem-1", created.Status);
        var state = await State(url, idempotent);
        Equal(2, state.Attempts);
        Same(new[] { "idem-1", "idem-1" }, state.Keys);
    }

    private static async Task ErrorsAsync(string url)
    {
        var expected = new (int Code, Type Kind)[]
        {
            (400, typeof(BadRequestException)),
            (401, typeof(UnauthorizedException)),
            (403, typeof(ForbiddenException)),
            (404, typeof(NotFoundException)),
            (409, typeof(ConflictException)),
            (422, typeof(UnprocessableEntityException)),
        };
        foreach (var (code, kind) in expected)
        {
            var capture = new Capture();
            using var counted = Make(url, capture);
            var error = await Throws<ApiException>(() => counted.Errors.RetrieveScenarioStatusAsync(code));
            Equal(kind, error.GetType());
            Equal(code, (int)error.StatusCode);
            var body = Declared(error);
            Equal($"status {code}", body.ErrorValue);
            Equal<int?>(code, body.Code);
            Equal("req_mock", error.RequestId);
            Equal(1, capture.Headers.Count);
        }

        // An error from a later page is not swallowed, and the iteration does not loop.
        var pages = new Capture();
        using var paged = Make(url, pages);
        var seen = new List<string>();
        var gone = await Throws<ConflictException>(async () =>
        {
            await foreach (var widget in paged.Errors.ListScenariosPagesAsync())
            {
                seen.Add(widget.Id);
            }
        });
        Same(new[] { "p1", "p2" }, seen);
        Equal(409, (int)gone.StatusCode);
        var page = Declared(gone);
        Equal("page_gone", page.ErrorValue);
        Equal<int?>(409, page.Code);
        Equal(2, pages.Headers.Count);

        // An error from the first page is raised by the first fetch.
        using var anonymous = new FeaturesClient(null, new() { BaseUrl = url });
        var first = await Throws<UnauthorizedException>(async () =>
        {
            await foreach (var widget in anonymous.Widgets.ListAsync())
            {
                seen.Add(widget.Id);
            }
        });
        Equal("req_mock", first.RequestId);
        await Throws<UnauthorizedException>(async () => await anonymous.Widgets.ListAsync());
        Equal(2, seen.Count);
    }

    private static async Task StreamingAsync(FeaturesClient client, string url)
    {
        var events = new List<SseEvent>();
        await using (var stream = await client.Streaming.RetrieveScenariosSseAsync())
        {
            await foreach (var sse in stream)
            {
                events.Add(sse);
            }
            Equal("7", stream.LastEventId);
        }
        // The comment line yields no event; the id persists, the retry applies to its own event.
        Equal(4, events.Count);
        Equal(new SseEvent("message", "first"), events[0]);
        Equal(new SseEvent("tick", "line1\nline2", "7"), events[1]);
        Equal(new SseEvent("message", "{\"n\": 3}", "7", 2500), events[2]);
        Equal(new SseEvent("message", "tail", "7"), events[3]);

        var capture = new Capture();
        using var counted = Make(url, capture);
        var denied = await Throws<ForbiddenException>(() => counted.Streaming.RetrieveScenariosSseErrorAsync());
        Equal(403, (int)denied.StatusCode);
        var body = Declared(denied);
        Equal("forbidden", body.ErrorValue);
        Equal<int?>(403, body.Code);
        Equal(1, capture.Headers.Count);
    }

    /// <summary>Cancelling a call that waits to retry stops it: the server sees one attempt only.</summary>
    private static async Task CancellationAsync(FeaturesClient client, string url)
    {
        var id = ScenarioId("cancel");
        using var cancel = new CancellationTokenSource(TimeSpan.FromMilliseconds(400));
        await Throws<OperationCanceledException>(
            () =>
                client.Retries.RetrieveScenariosRateLimitedAsync(
                    new() { XScenarioId = id },
                    cancellationToken: cancel.Token
                )
        );
        await Task.Delay(TimeSpan.FromSeconds(2.2));
        Equal(1, (await State(url, id)).Attempts);
    }
}

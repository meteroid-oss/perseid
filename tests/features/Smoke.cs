using System.Text;
using System.Text.Json;
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

using var client = new FeaturesClient("tok", new() { BaseUrl = serverUrl });
Equal("||", (await client.Account.CheckHealthAsync()).Status);
Equal("Bearer tok||", (await client.Account.RetrieveMachineAsync()).Status);
Ids("w1,w2,w3", await Collect(client.Widgets.ListAutoPagingAsync(), w => w.Id));
Ids(
    "e1,e2,e3",
    await Collect(client.Widgets.ListEventsAutoPagingAsync("w1", new() { Kind = "created" }), e => e.Id)
);
Ids("g1,g2,g3", await Collect(client.Gadgets.ListAutoPagingAsync(), g => g.Id));
Ids("r1,r2,r3", await Collect(client.Records.ListAutoPagingAsync(), r => r.Id));

var pages = new List<string>();
await foreach (var page in client.Widgets.ListAutoPagingAsync().AsPagesAsync())
{
    pages.Add($"{string.Join("+", page.Items.Select(w => w.Id))}:{page.HasNextPage}");
}
Equal("w1+w2:True,w3:False", string.Join(",", pages));
var first = await client.Gadgets.ListAutoPagingAsync().GetFirstPageAsync();
Equal(2, first.Response.Meta.TotalPages);
var second = await first.GetNextPageAsync();
Equal("g3", second.Items.Single().Id);
Equal(false, second.HasNextPage);

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
Ids("w1,w2,w3", await Collect(keyed.Widgets.ListAutoPagingAsync(), w => w.Id));
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
    }
);
Equal(
    "expand[]=a&expand[]=b&filter[amount][gte]=5&filter[status]=open&ids=x&ids=y"
        + "&metadata[k]=v&range[gte]=1&range[lt]=9&tags=t1,t2",
    searched.Status
);
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
    (await client.Wire.BetaSearchAsync(new() { Limit = 2, Features = "x,y" })).Status
);
Equal(
    "42:image/png:png",
    (await client.Wire.UpdateImageAsync("42", Encoding.UTF8.GetBytes("png"))).Status
);
Console.WriteLine("csharp smoke test passed");

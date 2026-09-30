using System.Text;
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
Equal("||", (await client.Account.HealthAsync()).Status);
Equal("Bearer tok||", (await client.Account.MachineStatusAsync()).Status);
Ids("w1,w2,w3", await Collect(client.Widgets.ListWidgetsIterAsync(), w => w.Id));
Ids(
    "e1,e2,e3",
    await Collect(client.Widgets.ListWidgetEventsIterAsync("w1", new() { Kind = "created" }), e => e.Id)
);
Ids("g1,g2,g3", await Collect(client.Gadgets.ListGadgetsIterAsync(), g => g.Id));
Ids("r1,r2,r3", await Collect(client.Records.ListRecordsIterAsync(), r => r.Id));

using var basic = new FeaturesClient(
    null,
    new() { BaseUrl = serverUrl, BasicAuth = new("u", "p") }
);
Equal("Basic dTpw||", (await basic.Account.CreateSessionAsync()).Status);

using var provided = new FeaturesClient(
    null,
    new() { BaseUrl = serverUrl, TokenProvider = _ => ValueTask.FromResult("fresh") }
);
Equal("Bearer fresh||", (await provided.Account.MachineStatusAsync()).Status);

using var keyed = new FeaturesClient(
    null,
    new() { BaseUrl = serverUrl, ApiKeys = new() { ApiKey = "k" } }
);
Ids("w1,w2,w3", await Collect(keyed.Widgets.ListWidgetsIterAsync(), w => w.Id));
using var anonymous = new FeaturesClient(null, new() { BaseUrl = serverUrl });
try
{
    await anonymous.Widgets.ListWidgetsAsync();
    throw new Exception("an unauthenticated call must fail");
}
catch (ApiException e) when ((int)e.StatusCode == 401) { }

var events = new List<SseEvent>();
await using (var stream = await client.Streaming.StreamEventsAsync(new() { Topic = "news" }))
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

var uploaded = await client.Streaming.UploadFileAsync(
    new()
    {
        File = Upload.FromBytes(Encoding.UTF8.GetBytes("hello"), "a.txt", "text/plain"),
        Name = "doc",
        Count = 2,
        Meta = new Health { Status = "ok" },
    }
);
Equal(
    "count=::2;file=a.txt:text/plain:hello;meta=:application/json:{\"status\":\"ok\"};name=::doc",
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
Console.WriteLine("csharp smoke test passed");

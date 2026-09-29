using System.Collections.Specialized;
using System.Net;
using System.Net.Sockets;
using System.Text;
using System.Text.Json;
using Petstore;
using Petstore.Models;

var port = FreePort();
using var server = new HttpListener();
server.Prefixes.Add($"http://127.0.0.1:{port}/");
server.Start();
var seen = new List<(string Method, string Url, NameValueCollection Headers, string Body)>();
var flaky = 0;
_ = Task.Run(async () =>
{
    while (server.IsListening)
    {
        var context = await server.GetContextAsync();
        var request = context.Request;
        var body = await new StreamReader(request.InputStream).ReadToEndAsync();
        seen.Add((request.HttpMethod, request.RawUrl!, request.Headers, body));
        var (status, reply) = (request.HttpMethod, request.Url!.AbsolutePath) switch
        {
            ("POST", "/pets") => (
                201,
                """{"id":"p1","name":"Rex","status":"adopted","created_at":"2024-01-02T03:04:05Z","extra":true}"""
            ),
            ("GET", "/pets") => (200, """{"data":[]}"""),
            ("GET", "/pets/flaky") => flaky++ == 0
                ? (503, "busy")
                : (200, """{"id":"flaky","name":"F","created_at":"2024-01-02T03:04:05+02:00"}"""),
            ("DELETE", "/pets/p1") => (204, ""),
            ("GET", "/pets/slow") => await Task.Delay(1000).ContinueWith(_ => (200, "{}")),
            _ => (404, """{"code":"not_found"}"""),
        };
        context.Response.StatusCode = status;
        var bytes = Encoding.UTF8.GetBytes(reply);
        await context.Response.OutputStream.WriteAsync(bytes);
        context.Response.Close();
    }
});

using var client = new PetstoreClient(
    "sk_test",
    new PetstoreClientOptions { BaseUrl = $"http://127.0.0.1:{port}" }
);

var created = await client.Pets.CreatePetAsync(
    new PetCreate { Name = "Rex", Status = PetStatus.Available }
);
var post = seen[^1];
Check(post.Body == """{"name":"Rex","status":"available"}""", $"request body {post.Body}");
Check(post.Headers["authorization"] == "Bearer sk_test", "bearer token");
Check(post.Headers["idempotency-key"]?.StartsWith("auto_") == true, "idempotency key");
Check(post.Headers["petstore-req-id"] is not null, "request id");
Check(
    post.Headers["user-agent"]?.StartsWith("petstore-csharp/") == true,
    $"user agent {post.Headers["user-agent"]}"
);
Check(
    created.Status == new PetStatus("adopted") && created.Status?.IsKnown == false,
    "unknown enum value kept"
);
Check(created.CreatedAt == new DateTimeOffset(2024, 1, 2, 3, 4, 5, TimeSpan.Zero), "date-time");
var json = JsonSerializer.Serialize(created, PetstoreJsonContext.Default.Pet);
Check(json.Contains("\"status\":\"adopted\"") && !json.Contains("\"tag\""), $"round trip {json}");
Check(
    JsonSerializer.Deserialize(json, PetstoreJsonContext.Default.Pet) == created,
    "model equality"
);

await client.Pets.ListPetsAsync(new PetsListPetsOptions { Limit = 10, Status = PetStatus.Sold });
Check(seen[^1].Url == "/pets?limit=10&status=sold", $"query {seen[^1].Url}");

var retried = await client.Pets.GetPetAsync("flaky");
Check(retried.Id == "flaky", "retried response");
Check(seen[^1].Headers["petstore-retry-count"] == "1", "retry count header");
Check(
    seen[^1].Headers["petstore-req-id"] == seen[^2].Headers["petstore-req-id"],
    "request id kept across retries"
);

await client.Pets.DeletePetAsync("p1");
try
{
    await client.Pets.GetPetAsync("missing/one");
    Check(false, "404 must throw");
}
catch (ApiException e)
{
    Check(e.StatusCode == HttpStatusCode.NotFound && e.Body.Contains("not_found"), "ApiException");
    Check(seen[^1].Url == "/pets/missing%2Fone", $"path escaping {seen[^1].Url}");
}

using var cancelled = new CancellationTokenSource();
cancelled.Cancel();
try
{
    await client.Pets.GetPetAsync("p1", cancelled.Token);
    Check(false, "cancellation must throw");
}
catch (OperationCanceledException) { }

using var impatient = new PetstoreClient(
    null,
    new PetstoreClientOptions
    {
        BaseUrl = $"http://127.0.0.1:{port}",
        Timeout = TimeSpan.FromMilliseconds(100),
        NumRetries = 0,
    }
);
try
{
    await impatient.Pets.GetPetAsync("slow");
    Check(false, "timeout must throw");
}
catch (TimeoutException) { }

Console.WriteLine("csharp smoke test passed");
return 0;

static void Check(bool condition, string what)
{
    if (!condition)
    {
        Console.Error.WriteLine($"FAILED: {what}");
        Environment.Exit(1);
    }
}

static int FreePort()
{
    var listener = new TcpListener(IPAddress.Loopback, 0);
    listener.Start();
    var port = ((IPEndPoint)listener.LocalEndpoint).Port;
    listener.Stop();
    return port;
}

using System.Net;
using System.Text;
using Audio;

/// <summary>The multipart `_stream` twin, on the SDK generated from tests/sdk/csharp/_twins/openapi.yaml.</summary>
public class TwinsTests
{
    private sealed class Server : HttpMessageHandler
    {
        public List<string> Bodies { get; } = [];

        protected override async Task<HttpResponseMessage> SendAsync(
            HttpRequestMessage request,
            CancellationToken cancellationToken
        )
        {
            Bodies.Add(await request.Content!.ReadAsStringAsync(cancellationToken));
            return request.Headers.Accept.ToString() == "text/event-stream"
                ? new(HttpStatusCode.OK)
                {
                    Content = new StringContent("data: {\"text\":\"hi\"}\n\n", Encoding.UTF8, "text/event-stream"),
                }
                : new(HttpStatusCode.OK)
                {
                    Content = new StringContent("""{"text":"hi"}""", Encoding.UTF8, "application/json"),
                };
        }
    }

    [Fact]
    public async Task OnlyTheStreamTwinSendsStream()
    {
        var server = new Server();
        using var client = new AudioClient("key", new() { HttpMessageHandler = server });
        await client.Audio.CreateTranscriptionAsync(new() { File = new byte[] { 1 } });
        var texts = new List<string>();
        await foreach (var delta in await client.Audio.CreateTranscriptionStreamAsync(new() { File = new byte[] { 1 } }))
        {
            texts.Add(delta.Text);
        }
        Assert.Equal(["hi"], texts);
        Assert.DoesNotContain("name=\"stream\"", server.Bodies[0], StringComparison.Ordinal);
        Assert.Matches("name=\"stream\"\r\n(.+\r\n)*\r\ntrue\r\n", server.Bodies[1]);
    }
}

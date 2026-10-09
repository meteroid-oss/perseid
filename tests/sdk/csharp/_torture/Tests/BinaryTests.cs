using System.Net;
using System.Text;
using Torture;

public class BinaryTests
{
    /// <summary>A body of parts, each after <c>pause</c> but the first, failing past them when
    /// <c>truncated</c>.</summary>
    private sealed class Parts(Func<CancellationToken, Task> pause, bool truncated, params string[] parts) : Stream
    {
        private int _next;
        private byte[] _current = [];
        private int _offset;

        public override bool CanRead => true;
        public override bool CanSeek => false;
        public override bool CanWrite => false;
        public override long Length => throw new NotSupportedException();

        public override long Position
        {
            get => throw new NotSupportedException();
            set => throw new NotSupportedException();
        }

        public override async ValueTask<int> ReadAsync(Memory<byte> buffer, CancellationToken cancellationToken = default)
        {
            if (_offset == _current.Length)
            {
                if (_next == parts.Length)
                {
                    return truncated ? throw new IOException("the connection was reset") : 0;
                }
                if (_next > 0)
                {
                    await pause(cancellationToken);
                }
                _current = Encoding.UTF8.GetBytes(parts[_next++]);
                _offset = 0;
            }
            var count = Math.Min(buffer.Length, _current.Length - _offset);
            _current.AsMemory(_offset, count).CopyTo(buffer);
            _offset += count;
            return count;
        }

        public override Task<int> ReadAsync(byte[] buffer, int offset, int count, CancellationToken cancellationToken) =>
            ReadAsync(buffer.AsMemory(offset, count), cancellationToken).AsTask();

        public override int Read(byte[] buffer, int offset, int count) => ReadAsync(buffer, offset, count).GetAwaiter().GetResult();
        public override void Flush() { }
        public override long Seek(long offset, SeekOrigin origin) => throw new NotSupportedException();
        public override void SetLength(long value) => throw new NotSupportedException();
        public override void Write(byte[] buffer, int offset, int count) => throw new NotSupportedException();
    }

    private sealed class Server(Func<int, HttpResponseMessage> respond) : HttpMessageHandler
    {
        public int Attempts;

        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken) =>
            Task.FromResult(respond(Interlocked.Increment(ref Attempts)));
    }

    private static HttpResponseMessage Pdf(Stream body)
    {
        var content = new StreamContent(body);
        content.Headers.ContentType = new("application/pdf");
        var response = new HttpResponseMessage(HttpStatusCode.OK) { Content = content };
        response.Headers.Add("Content-Disposition-Hint", "t1.pdf");
        return response;
    }

    private static Func<CancellationToken, Task> Delay(int milliseconds) => token => Task.Delay(milliseconds, token);

    private static (TortureClient, Server) Client(Func<int, HttpResponseMessage> respond, TimeSpan? timeout = null)
    {
        var server = new Server(respond);
        var options = new TortureClientOptions
        {
            BaseUrl = "https://torture.test/v1",
            HttpMessageHandler = server,
            RetrySchedule = [TimeSpan.Zero, TimeSpan.Zero],
        };
        if (timeout is { } value)
        {
            options.Timeout = value;
        }
        return (new TortureClient("token", options), server);
    }

    [Fact]
    public async Task TheBodyStreamsBeforeItEnds()
    {
        var gate = new TaskCompletionSource();
        var (client, _) = Client(_ => Pdf(new Parts(token => gate.Task.WaitAsync(token), false, "%PDF-", "rest")));
        using var _ = client;
        await using var body = await client.Things.DownloadAsync("t1");
        Assert.Equal("application/pdf", body.ContentType);
        Assert.Contains("t1.pdf", body.Headers.GetValues("Content-Disposition-Hint"));
        var stream = await body.OpenStreamAsync();
        var first = new byte[5];
        await stream.ReadExactlyAsync(first);
        Assert.Equal("%PDF-", Encoding.UTF8.GetString(first));
        gate.SetResult();
        Assert.Equal("rest", Encoding.UTF8.GetString(await body.ReadAsBytesAsync()));
    }

    [Fact]
    public async Task TheBodyIsReadWholeCopiedOrWrittenToAFile()
    {
        var (client, _) = Client(_ => Pdf(new Parts(_ => Task.CompletedTask, false, "%PDF-", "1.7")));
        using var _ = client;
        Assert.Equal("%PDF-1.7", Encoding.UTF8.GetString(await client.Things.DownloadAsync("t1").ReadAsBytesAsync()));

        var path = Path.GetTempFileName();
        try
        {
            await client.Things.DownloadAsync("t1").WriteToFileAsync(path);
            Assert.Equal("%PDF-1.7", await File.ReadAllTextAsync(path));
        }
        finally
        {
            File.Delete(path);
        }

        using var copy = new MemoryStream();
        await (await client.Things.DownloadAsync("t1")).CopyToAsync(copy);
        Assert.Equal("%PDF-1.7", Encoding.UTF8.GetString(copy.ToArray()));

        var raw = await client.Things.WithRawResponse.DownloadAsync("t1");
        await using var rawBody = raw.Value;
        Assert.Equal(HttpStatusCode.OK, raw.StatusCode);
        Assert.Equal("%PDF-1.7", Encoding.UTF8.GetString(await rawBody.ReadAsBytesAsync()));
    }

    [Fact]
    public async Task AFailedDownloadLeavesTheFileAsItWas()
    {
        var (client, _) = Client(n => Pdf(new Parts(_ => Task.CompletedTask, n == 1, "%PDF-", "1.7")));
        using var _ = client;
        var dir = Directory.CreateTempSubdirectory();
        try
        {
            var path = Path.Combine(dir.FullName, "t1.pdf");
            await File.WriteAllTextAsync(path, "old");
            await Assert.ThrowsAsync<ApiConnectionException>(() => client.Things.DownloadAsync("t1").WriteToFileAsync(path));
            Assert.Equal("old", await File.ReadAllTextAsync(path));
            await client.Things.DownloadAsync("t1").WriteToFileAsync(path);
            Assert.Equal("%PDF-1.7", await File.ReadAllTextAsync(path));
            Assert.Equal([path], Directory.GetFiles(dir.FullName));
        }
        finally
        {
            dir.Delete(recursive: true);
        }
    }

    [Fact]
    public async Task AnErrorStatusIsThrownBeforeTheBody()
    {
        var (client, server) = Client(_ =>
            new HttpResponseMessage(HttpStatusCode.NotFound)
            {
                Content = new StringContent("""{"message":"no such thing"}""", Encoding.UTF8, "application/json"),
            }
        );
        using var _ = client;
        var error = await Assert.ThrowsAsync<NotFoundException>(() => client.Things.DownloadAsync("t1"));
        Assert.Contains("no such thing", error.Message);
        Assert.Equal(1, server.Attempts);
    }

    [Fact]
    public async Task RetriesStopOnceTheHeadersArrive()
    {
        var (client, server) = Client(n =>
            n == 1
                ? new HttpResponseMessage(HttpStatusCode.ServiceUnavailable)
                : Pdf(new Parts(_ => Task.CompletedTask, true, "%PDF-"))
        );
        using var _ = client;
        var body = await client.Things.DownloadAsync("t1");
        Assert.Equal(2, server.Attempts);
        await Assert.ThrowsAsync<ApiConnectionException>(() => body.ReadAsBytesAsync());
        Assert.Equal(2, server.Attempts);
    }

    [Fact]
    public async Task TheTimeoutBoundsEachReadNotTheDownload()
    {
        var (client, _) = Client(
            n => Pdf(new Parts(Delay(n == 1 ? 50 : 5000), false, "x", "x", "x", "x", "x", "x")),
            TimeSpan.FromMilliseconds(200)
        );
        using var _ = client;
        Assert.Equal(6, (await client.Things.DownloadAsync("t1").ReadAsBytesAsync()).Length);
        await Assert.ThrowsAsync<ApiTimeoutException>(() => client.Things.DownloadAsync("t1").ReadAsBytesAsync());
    }

    [Fact]
    public async Task TheCallsTokenCancelsTheReads()
    {
        var (client, _) = Client(_ => Pdf(new Parts(Delay(5000), false, "%PDF-", "never")));
        using var _ = client;
        using var cancel = new CancellationTokenSource();
        await using var body = await client.Things.DownloadAsync("t1", cancellationToken: cancel.Token);
        cancel.CancelAfter(50);
        await Assert.ThrowsAnyAsync<OperationCanceledException>(() => body.ReadAsBytesAsync());
    }
}

using System.Net.Http.Headers;
using System.Text;
using Petstore;

public class WebhookTests
{
    private const string Secret = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw";
    private const string Payload = """{"test": 2432232314}""";

    private static Dictionary<string, string> Headers(
        string prefix,
        string signature,
        DateTimeOffset timestamp
    ) =>
        new()
        {
            [$"{prefix}-id"] = "msg_1",
            [$"{prefix}-signature"] = signature,
            [$"{prefix}-timestamp"] = timestamp.ToUnixTimeSeconds().ToString(),
        };

    [Fact]
    public void SignsLikeTheStandardWebhooksTestVector()
    {
        var signature = new Webhook(Secret).Sign(
            "msg_p5jXN8AQM9LWM0D4loKWxJek",
            DateTimeOffset.FromUnixTimeSeconds(1614265330),
            Payload
        );
        Assert.Equal("v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE=", signature);
    }

    [Fact]
    public void VerifiesBothHeaderFamiliesAndHeaderContainers()
    {
        var webhook = new Webhook(Secret);
        var now = DateTimeOffset.UtcNow;
        var signature = webhook.Sign("msg_1", now, Payload);
        foreach (var prefix in new[] { "webhook", "svix", "Webhook" })
        {
            var headers = Headers(prefix, signature, now);
            webhook.Verify(Payload, headers);
            webhook.Verify(
                Encoding.UTF8.GetBytes(Payload),
                name =>
                    headers
                        .FirstOrDefault(h => h.Key.Equals(name, StringComparison.OrdinalIgnoreCase))
                        .Value
            );
            using var message = new HttpRequestMessage();
            foreach (var (name, value) in headers)
            {
                message.Headers.TryAddWithoutValidation(name, value);
            }
            webhook.Verify(Payload, message.Headers);
        }
        using var rotated = new HttpRequestMessage();
        foreach (var (name, value) in Headers("webhook", "v1,AAAA", now))
        {
            rotated.Headers.TryAddWithoutValidation(name, value);
        }
        rotated.Headers.TryAddWithoutValidation("webhook-signature", $"v2,{signature[3..]} {signature}");
        webhook.Verify(Payload, rotated.Headers);
    }

    [Fact]
    public void RejectsTamperedStaleAndIncompleteRequests()
    {
        var webhook = new Webhook(Secret);
        var now = DateTimeOffset.UtcNow;
        var signature = webhook.Sign("msg_1", now, Payload);
        void Fails(string payload, Dictionary<string, string> headers, string message) =>
            Assert.Contains(
                message,
                Assert.Throws<WebhookVerificationException>(() => webhook.Verify(payload, headers)).Message
            );

        Fails("tampered", Headers("webhook", signature, now), "No matching");
        Fails(Payload, [], "Missing");
        var invalid = Headers("webhook", signature, now);
        invalid["webhook-timestamp"] = "soon";
        Fails(Payload, invalid, "Invalid");
        var old = now.AddHours(-1);
        Fails(Payload, Headers("webhook", webhook.Sign("msg_1", old, Payload), old), "too old");
        var future = now.AddHours(1);
        Fails(Payload, Headers("webhook", webhook.Sign("msg_1", future, Payload), future), "too new");
    }

    [Fact]
    public void WebhookHeadersWinOverSvixHeaders()
    {
        var webhook = new Webhook(Secret);
        var now = DateTimeOffset.UtcNow;
        var mixed = Headers("svix", webhook.Sign("msg_1", now, Payload), now);
        foreach (var (name, value) in Headers("webhook", "v1,AAAA", now))
        {
            mixed[name] = value;
        }
        Assert.Throws<WebhookVerificationException>(() => webhook.Verify(Payload, mixed));
    }

    [Fact]
    public void RejectsUnusableSecrets()
    {
        Assert.Throws<ArgumentException>(() => new Webhook("whsec_!!!"));
        Assert.Throws<ArgumentException>(() => new Webhook("whsec_"));
        Assert.Throws<ArgumentException>(() => new Webhook(Array.Empty<byte>()));
    }
}

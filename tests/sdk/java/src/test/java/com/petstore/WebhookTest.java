package com.petstore;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;

import com.petstore.exceptions.WebhookVerificationException;
import java.time.Instant;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import org.junit.jupiter.api.Test;

class WebhookTest {
    private static final String SECRET = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw";
    private static final String PAYLOAD = "{\"test\": 2432232314}";

    private static Map<String, List<String>> headers(String prefix, String signature, String timestamp) {
        Map<String, List<String>> headers = new HashMap<>();
        headers.put(prefix + "-id", List.of("msg_1"));
        headers.put(prefix + "-signature", List.of(signature));
        headers.put(prefix + "-timestamp", List.of(timestamp));
        return headers;
    }

    // One instant for the whole run, so a signature and its timestamp never straddle a second.
    private static final long NOW = Instant.now().getEpochSecond();

    private static String now() {
        return String.valueOf(NOW);
    }

    private static void assertRejected(Webhook webhook, String payload, Map<String, List<String>> headers, String message) {
        WebhookVerificationException error =
                assertThrows(WebhookVerificationException.class, () -> webhook.verify(payload, headers));
        assertEquals(message, error.getMessage());
    }

    @Test
    void signsLikeTheStandardWebhooksTestVector() {
        assertEquals(
                "v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE=",
                new Webhook(SECRET).sign("msg_p5jXN8AQM9LWM0D4loKWxJek", 1614265330L, PAYLOAD));
    }

    @Test
    void verifiesBothHeaderFamilies() {
        Webhook webhook = new Webhook(SECRET);
        String signature = webhook.sign("msg_1", NOW, PAYLOAD);
        for (String prefix : List.of("webhook", "svix", "Webhook")) {
            assertDoesNotThrow(() -> webhook.verify(PAYLOAD, headers(prefix, signature, now())));
        }
    }

    @Test
    void rejectsBadRequests() {
        Webhook webhook = new Webhook(SECRET);
        long now = NOW;
        String good = webhook.sign("msg_1", now, PAYLOAD);
        assertRejected(webhook, "tampered", headers("webhook", good, now()), "No matching signature found");
        assertRejected(webhook, PAYLOAD, Map.of(), "Missing required headers");
        assertRejected(webhook, PAYLOAD, headers("webhook", good, "soon"), "Invalid Signature Headers");
        String old = webhook.sign("msg_1", now - 3600, PAYLOAD);
        assertRejected(webhook, PAYLOAD, headers("webhook", old, String.valueOf(now - 3600)), "Message timestamp too old");
        String future = webhook.sign("msg_1", now + 3600, PAYLOAD);
        assertRejected(webhook, PAYLOAD, headers("webhook", future, String.valueOf(now + 3600)), "Message timestamp too new");
    }

    @Test
    void standardHeadersWinAndRotatedSignaturesAreAccepted() {
        Webhook webhook = new Webhook(SECRET);
        String good = webhook.sign("msg_1", NOW, PAYLOAD);
        Map<String, List<String>> mixed = headers("svix", good, now());
        mixed.putAll(headers("webhook", "v1,AAAA", now()));
        assertRejected(webhook, PAYLOAD, mixed, "No matching signature found");
        assertDoesNotThrow(
                () -> webhook.verify(PAYLOAD, headers("webhook", "v1,AAAA v2," + good + " " + good, now())));
    }

    @Test
    void rejectsUnusableSecrets() {
        assertThrows(IllegalArgumentException.class, () -> new Webhook("whsec_!!!"));
        assertThrows(IllegalArgumentException.class, () -> new Webhook("whsec_"));
        assertThrows(IllegalArgumentException.class, () -> new Webhook(new byte[0]));
    }
}

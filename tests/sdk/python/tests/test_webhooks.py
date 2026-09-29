import time
import unittest

from petstore.webhooks import (
    InvalidWebhookSecretError,
    Webhook,
    WebhookVerificationError,
)

SECRET = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw"
PAYLOAD = '{"test": 2432232314}'


def headers(prefix, signature, timestamp=None):
    return {
        f"{prefix}-id": "msg_1",
        f"{prefix}-signature": signature,
        f"{prefix}-timestamp": str(int(time.time()) if timestamp is None else timestamp),
    }


class WebhookTest(unittest.TestCase):
    def setUp(self):
        self.webhook = Webhook(SECRET)

    def test_signs_like_the_standard_webhooks_test_vector(self):
        signature = self.webhook.sign("msg_p5jXN8AQM9LWM0D4loKWxJek", 1614265330, PAYLOAD)
        self.assertEqual(signature, "v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE=")

    def test_verifies_both_header_families(self):
        signature = self.webhook.sign("msg_1", int(time.time()), PAYLOAD)
        for prefix in ("webhook", "svix"):
            self.webhook.verify(PAYLOAD, headers(prefix, signature))
            self.webhook.verify(PAYLOAD.encode(), headers(prefix.title(), signature))

    def test_rejects_bad_requests(self):
        now = int(time.time())
        good = self.webhook.sign("msg_1", now, PAYLOAD)
        old = self.webhook.sign("msg_1", now - 3600, PAYLOAD)
        future = self.webhook.sign("msg_1", now + 3600, PAYLOAD)
        cases = [
            ("tampered", headers("webhook", good), "No matching"),
            (PAYLOAD, {}, "Missing"),
            (PAYLOAD, headers("webhook", good, "soon"), "Invalid"),
            (PAYLOAD, headers("webhook", old, now - 3600), "too old"),
            (PAYLOAD, headers("webhook", future, now + 3600), "too new"),
        ]
        for payload, request_headers, message in cases:
            with self.assertRaisesRegex(WebhookVerificationError, message):
                self.webhook.verify(payload, request_headers)

    def test_standard_headers_win_and_rotated_signatures_are_accepted(self):
        good = self.webhook.sign("msg_1", int(time.time()), PAYLOAD)
        mixed = {**headers("svix", good), **headers("webhook", "v1,AAAA")}
        with self.assertRaises(WebhookVerificationError):
            self.webhook.verify(PAYLOAD, mixed)
        self.webhook.verify(PAYLOAD, headers("webhook", f"v1,AAAA v2,{good} {good}"))

    def test_rejects_unusable_secrets(self):
        for secret in ("whsec_!!!", "whsec_", b""):
            with self.assertRaises(InvalidWebhookSecretError):
                Webhook(secret)


if __name__ == "__main__":
    unittest.main()

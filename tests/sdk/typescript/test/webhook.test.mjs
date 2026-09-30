import assert from "node:assert/strict";
import { test } from "node:test";
import { Webhook, WebhookVerificationError } from "../dist/esm/index.js";

const SECRET = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw";
const PAYLOAD = '{"test": 2432232314}';
const now = () => Math.floor(Date.now() / 1000);

function headers(prefix, signature, timestamp = now()) {
  return {
    [`${prefix}-id`]: "msg_1",
    [`${prefix}-signature`]: signature,
    [`${prefix}-timestamp`]: String(timestamp),
  };
}

test("signs like the Standard Webhooks test vector", () => {
  const signature = new Webhook(SECRET).sign(
    "msg_p5jXN8AQM9LWM0D4loKWxJek",
    new Date(1614265330 * 1000),
    PAYLOAD
  );
  assert.equal(signature, "v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE=");
});

test("verifies both header families and header containers", () => {
  const webhook = new Webhook(SECRET);
  const signature = webhook.sign("msg_1", new Date(), PAYLOAD);
  for (const prefix of ["webhook", "svix"]) {
    webhook.verify(PAYLOAD, headers(prefix, signature));
    webhook.verify(Buffer.from(PAYLOAD), new Headers(headers(prefix, signature)));
  }
  webhook.verify(PAYLOAD, { ...headers("Webhook", signature), other: undefined });
  webhook.verify(PAYLOAD, { ...headers("webhook", ""), "webhook-signature": ["v1,AAAA", signature] });
});

test("rejects tampered, stale and incomplete requests", () => {
  const webhook = new Webhook(SECRET);
  const signature = webhook.sign("msg_1", new Date(), PAYLOAD);
  const fails = (payload, h, message) =>
    assert.throws(
      () => webhook.verify(payload, h),
      (error) => error instanceof WebhookVerificationError && message.test(error.message)
    );

  fails("tampered", headers("webhook", signature), /No matching/);
  fails(PAYLOAD, {}, /Missing/);
  fails(PAYLOAD, headers("webhook", signature, "soon"), /Invalid/);
  const old = now() - 3600;
  fails(PAYLOAD, headers("webhook", webhook.sign("msg_1", new Date(old * 1000), PAYLOAD), old), /too old/);
  const future = now() + 3600;
  fails(PAYLOAD, headers("webhook", webhook.sign("msg_1", new Date(future * 1000), PAYLOAD), future), /too new/);
});

test("webhook headers win over svix headers", () => {
  const webhook = new Webhook(SECRET);
  const signature = webhook.sign("msg_1", new Date(), PAYLOAD);
  const mixed = { ...headers("svix", signature), ...headers("webhook", "v1,AAAA") };
  assert.throws(() => webhook.verify(PAYLOAD, mixed), WebhookVerificationError);
});

test("rejects unusable secrets", () => {
  assert.throws(() => new Webhook("whsec_!!!"));
  assert.throws(() => new Webhook("whsec_"));
  assert.throws(() => new Webhook(new Uint8Array()));
});

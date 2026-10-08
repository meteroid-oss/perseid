// Generates tests/fixtures/realworld.yaml with resource-style method names, then checks the
// primitive-or-object unions and the typed error bodies of a Stripe-like API.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { copyFileSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { after, describe, it } from "node:test";
import { pathToFileURL } from "node:url";

const fixtures = process.env.FIXTURES;
assert.ok(fixtures, "set FIXTURES to the perseid tests/fixtures directory");
const dir = mkdtempSync(join(tmpdir(), "perseid-realworld-"));
after(() => rmSync(dir, { recursive: true, force: true }));
copyFileSync(join(fixtures, "realworld.yaml"), join(dir, "openapi.yaml"));
writeFileSync(
  join(dir, "perseid.toml"),
  'spec = "openapi.yaml"\nsdks = ["typescript"]\nname = "RealWorld"\nbase_url = ""\ntimeout = 15\n[typescript]\n'
);
execFileSync("perseid", ["generate"], { cwd: dir, stdio: "ignore" });
const sdkDir = join(dir, "typescript");
symlinkSync(resolve("node_modules"), join(sdkDir, "node_modules"));
execFileSync("npm", ["run", "build"], { cwd: sdkDir, stdio: "ignore" });
const sdk = await import(pathToFileURL(join(sdkDir, "dist/esm/index.js")).href);

const CUSTOMER = { email: "a@b.c", id: "cus_1", object: "customer" };

function client(responses) {
  const calls = [];
  const fetch = async (url, init) => {
    calls.push({ url: new URL(url), init });
    return responses[Math.min(calls.length - 1, responses.length - 1)].clone();
  };
  const realWorld = new sdk.RealWorld({ apiKey: "sk_test", baseURL: "https://api.test", fetch, maxRetries: 0 });
  return { calls, realWorld };
}

describe("unions", () => {
  it("parse an expandable field as its id or as the model", () => {
    const expanded = sdk.ChargeSerializer.parse({ id: "ch_1", customer: CUSTOMER, amount: "" });
    assert.equal(expanded.customer.email, "a@b.c");
    assert.equal(expanded.amount, "");
    const id = sdk.ChargeSerializer.parse({ id: "ch_1", customer: "cus_1", tags: "a" });
    assert.equal(id.customer, "cus_1");
    assert.equal(id.tags, "a");
    assert.equal(sdk.ChargeSerializer.parse({ id: "ch_1", customer: null }).customer, null);
  });

  it("serialize each variant back to its wire form", () => {
    for (const json of [
      { id: "ch_1", customer: CUSTOMER, shipping: { name: "n" }, tags: ["a"] },
      { id: "ch_1", customer: "cus_1", shipping: "", amount: 3 },
    ]) {
      const charge = sdk.ChargeSerializer.parse(json);
      assert.deepEqual(JSON.parse(JSON.stringify(sdk.ChargeSerializer.serialize(charge))), json);
    }
  });
});

describe("client", () => {
  it("names methods after their resource action", async () => {
    const { calls, realWorld } = client([Response.json({ id: "ch_1", customer: CUSTOMER })]);
    const charge = await realWorld.charges.retrieve("ch_1");
    assert.equal(charge.customer.id, "cus_1");
    assert.equal(calls[0].url.pathname, "/v1/charges/ch_1");
    assert.equal(typeof realWorld.charges.capture, "function");
    assert.equal(realWorld.charges, realWorld.charges);
  });

  it("requires a base URL when perseid.toml sets `base_url = \"\"`", () => {
    assert.throws(
      () => new sdk.RealWorld({ apiKey: "sk_test" }),
      (error) =>
        error instanceof sdk.RealWorldError &&
        error.message.includes("`baseURL`") &&
        error.message.includes("`REAL_WORLD_BASE_URL`")
    );
    process.env.REAL_WORLD_BASE_URL = "https://env.test";
    try {
      assert.ok(new sdk.RealWorld({ apiKey: "sk_test" }));
    } finally {
      delete process.env.REAL_WORLD_BASE_URL;
    }
  });

  it("types the error body with the API-wide error schema", async () => {
    const error = { error: { message: "No such charge", type: "invalid_request_error" } };
    const { realWorld } = client([Response.json(error, { status: 404, headers: { "request-id": "req_9" } })]);
    await assert.rejects(realWorld.charges.retrieve("ch_x"), (thrown) => {
      assert.ok(thrown instanceof sdk.NotFoundError);
      assert.equal(thrown.error.error.message, "No such charge");
      assert.equal(thrown.requestId, "req_9");
      return true;
    });
  });
});

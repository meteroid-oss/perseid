// Generates tests/fixtures/torture.yaml with `int64 = "bigint"`, then round-trips its models
// and drives the client against a fake fetch.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { copyFileSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { after, describe, it } from "node:test";
import { pathToFileURL } from "node:url";

const fixtures = process.env.FIXTURES;
assert.ok(fixtures, "set FIXTURES to the perseid tests/fixtures directory");
const dir = mkdtempSync(join(tmpdir(), "perseid-torture-"));
after(() => rmSync(dir, { recursive: true, force: true }));
copyFileSync(join(fixtures, "torture.yaml"), join(dir, "openapi.yaml"));
writeFileSync(
  join(dir, "perseid.toml"),
  'spec = "openapi.yaml"\nname = "Torture"\nbase_url = "https://torture.test/v1"\n[typescript]\nint64 = "bigint"\ntyped_unions = true\n'
);
execFileSync("perseid", ["init"], { cwd: dir, stdio: "ignore" });
execFileSync("perseid", ["generate"], { cwd: dir, stdio: "ignore" });
const sdkDir = join(dir, "typescript");
symlinkSync(resolve("node_modules"), join(sdkDir, "node_modules"));
execFileSync("npm", ["run", "build"], { cwd: sdkDir, stdio: "ignore" });
const sdk = await import(pathToFileURL(join(sdkDir, "dist/esm/index.js")).href);

const SAMPLES = {
  Thing: `{"id":"t1","name":"n","created_at":"2024-01-02T03:04:05.123Z","kind":"beta-2",
    "count":9007199254740993,"unsigned":18446744073709551615,"nullable_required":null,
    "nullable_optional":null,"nullable_ref":null,"tags":["a"],"metadata":{"k":"v"},
    "attrs":{"x":[1,2]},"amount":"12.50","birthday":"2024-02-29","priority":10,
    "nested_map":{"a":[{"line1":"l"}]},"anyof_nullable_ref":{"line1":"x"}}`,
  Shape: '{"type":"circle","radius":1.5}',
  Pet: '{"pet_type":"Cat","meow":true}',
  Composed: '{"id":"b1","created_at":"2024-01-02T03:04:05.000Z","extra":"e","sibling_prop":"s"}',
  TreeNode: '{"value":"root","children":[{"value":"c","children":[]}],"parent":null}',
  UnionHolder: `{"shape":{"type":"circle","radius":1},"shapes":[{"type":"square","side":1}],
    "maybe_shape":null,"shape_map":{"k":{"type":"square","side":3}},"inline_union":["a"],
    "empty":{},"free_form":{"any":1},"counts":{"a":9007199254740993},
    "nested":{"id":"n","depth":2}}`,
  ThingPatch: '{"description":null,"count":null}',
  Activity: '{"kind":"reopened","at":"2024-01-02T03:04:05.000Z"}',
};

function roundTrip(model, text) {
  const serializer = sdk[`${model}Serializer`];
  return sdk.parseJson(sdk.stringifyJson(serializer.serialize(serializer.parse(sdk.parseJson(text)))));
}

describe("models", () => {
  for (const [model, text] of Object.entries(SAMPLES)) {
    it(`round-trips ${model}`, () => {
      assert.deepEqual(roundTrip(model, text), sdk.parseJson(text));
    });
  }

  it("reads int64 values as exact bigints", () => {
    const thing = sdk.ThingSerializer.parse(sdk.parseJson(SAMPLES.Thing));
    assert.equal(thing.count, 9007199254740993n);
    assert.equal(thing.unsigned, 18446744073709551615n);
    assert.equal(thing.createdAt.toISOString(), "2024-01-02T03:04:05.123Z");
    assert.equal(thing.nullableRef, null);
  });

  it("merges allOf parts into one object", () => {
    const composed = sdk.ComposedSerializer.parse(sdk.parseJson(SAMPLES.Composed));
    assert.deepEqual(Object.keys(composed).sort(), ["createdAt", "extra", "id", "siblingProp"]);
  });

  it("keeps unknown union variants and enum values as received", () => {
    const triangle = { type: "triangle", sides: [3, 4, 5] };
    assert.deepEqual(sdk.ShapeSerializer.serialize(sdk.ShapeSerializer.parse(triangle)), triangle);
    const kind = sdk.ThingSerializer.parse({ ...sdk.parseJson(SAMPLES.Thing), kind: "new" }).kind;
    assert.equal(kind, "new");
  });

  it("names union variants after their discriminator values", () => {
    const activity = sdk.ActivitySerializer.parse(sdk.parseJson(SAMPLES.Activity));
    assert.equal(activity.kind, "reopened");
    assert.equal(activity.at.toISOString(), "2024-01-02T03:04:05.000Z");
  });

  it("fills the discriminator of a union variant model", () => {
    assert.equal(sdk.CircleSerializer.serialize({ radius: 1 }).type, "circle");
    assert.equal(sdk.CircleSerializer.parse({ radius: 1 }).type, "circle");
  });

  it("picks the variant of a union of objects and keeps unknown shapes", () => {
    const parse = (value) => sdk.ObjectUnionsSerializer.parse(value);
    const account = parse({ account: { id: "a1", object: "account", email: "e" } }).account;
    const deleted = parse({ account: { deleted: true, id: "a2", object: "account" } }).account;
    const unknown = { object: "account_v2", id: "a3" };
    assert.ok("email" in account && !("deleted" in account));
    assert.equal(deleted.deleted, true);
    assert.ok(!("email" in deleted));
    assert.deepEqual(parse({ account: unknown }).account, unknown);
    assert.equal(sdk.expandableId(deleted), "a2");
    assert.equal(sdk.expandableId(parse({ account: "a0" }).account), "a0");

    const unions = parse({
      source: { file_id: "f" },
      sources: [{ url: "u", detail: "d" }, { path: "p" }],
      document: { title: "t", author: "a" },
      loose: { title: "t" },
    });
    assert.equal(unions.source.fileId, "f");
    assert.deepEqual(unions.sources[1], { path: "p" });
    assert.ok("publishedAt" in unions.document);
    assert.ok(!("author" in parse({ document: { title: "t" } }).document), "ties go to the first variant");
    assert.deepEqual(parse({ document: { body: "b" } }).document, { body: "b" });
    for (const text of [
      '{"account":{"deleted":true,"id":"a2","object":"account"}}',
      '{"account":{"object":"account_v2","id":"a3"},"sources":[{"file_id":"f"},{"path":"p"}]}',
      '{"source":{"url":"u"},"document":{"title":"t","author":"a"},"loose":{"title":"t"}}',
    ]) {
      assert.deepEqual(roundTrip("ObjectUnions", text), sdk.parseJson(text));
    }
  });

  it("sends explicit nulls to clear nullable fields", () => {
    assert.deepEqual(sdk.ThingPatchSerializer.serialize({ description: null, count: null }), {
      name: undefined,
      description: null,
      count: null,
    });
  });
});

const THING = sdk.parseJson(SAMPLES.Thing);

function client(responses, options = {}) {
  const calls = [];
  const fetch = async (url, init) => {
    calls.push({ url: new URL(url), init });
    const next = responses[Math.min(calls.length - 1, responses.length - 1)];
    return typeof next === "function" ? next(init) : next.clone();
  };
  return { calls, torture: new sdk.Torture("token", { fetch, ...options }) };
}

const json = (body, status = 200, headers = {}) =>
  new Response(sdk.stringifyJson(body), { status, headers });

describe("client", () => {
  it("keeps the base URL path and sends bigints losslessly", async () => {
    const { calls, torture } = client([new Response(SAMPLES.Thing)]);
    const thing = await torture.things.updateThing("a/b", { count: 9007199254740993n });
    assert.equal(calls[0].url.pathname, "/v1/things/a%2Fb");
    assert.equal(calls[0].init.body, '{"count":9007199254740993}');
    assert.equal(thing.count, 9007199254740993n);
  });

  it("ignores a trailing slash in the server URL", async () => {
    const { calls, torture } = client([new Response(SAMPLES.Thing)], {
      serverUrl: "https://torture.test/v2/",
    });
    await torture.things.getThing("t");
    assert.equal(calls[0].url.pathname, "/v2/things/t");
  });

  it("sends a user Idempotency-Key instead of the automatic one, whatever its case", async () => {
    const body = { name: "n", kind: "alpha" };
    const { calls, torture } = client([json(THING, 201)]);
    await torture.things.createThing(body, { idempotencyKey: "mine" });
    await torture.things.createThing(body, undefined, { headers: { "IDEMPOTENCY-KEY": "theirs" } });
    await torture.things.createThing(body);
    assert.equal(calls[0].init.headers["idempotency-key"], "mine");
    assert.equal(calls[1].init.headers["idempotency-key"], "theirs");
    assert.match(calls[2].init.headers["idempotency-key"], /^auto_[0-9a-f-]{36}$/);
    for (const call of calls) {
      assert.equal(Object.keys(call.init.headers).filter((h) => /idempotency/i.test(h)).length, 1);
    }
  });

  it("takes per-request headers", async () => {
    const { calls, torture } = client([json(THING)]);
    await torture.things.getThing("t1", { headers: new Headers({ "X-Extra": "1" }) });
    assert.equal(calls[0].init.headers["x-extra"], "1");
    assert.equal(calls[0].init.headers.authorization, "Bearer token");
  });

  it("retries a 429 after its Retry-After delay", async () => {
    const { calls, torture } = client([
      json({}, 429, { "retry-after": "0" }),
      json({}, 503, { "retry-after": "0" }),
      json(THING),
    ]);
    const thing = await torture.things.getThing("t1");
    assert.equal(thing.id, "t1");
    assert.equal(calls.length, 3);
    assert.equal(calls[2].init.headers["torture-retry-count"], "2");
  });

  it("does not retry a failed PATCH without an idempotency key", async () => {
    const { calls, torture } = client([json({}, 500)], { retryScheduleInMs: [1, 1] });
    await assert.rejects(torture.things.updateThing("t1", {}), (error) => {
      assert.ok(error instanceof sdk.ApiException);
      assert.equal(error.status, 500);
      return true;
    });
    assert.equal(calls.length, 1);
  });

  it("stops retrying when the caller aborts", async () => {
    const controller = new AbortController();
    const { calls, torture } = client([
      () => {
        controller.abort();
        return json({}, 503);
      },
    ]);
    await assert.rejects(
      torture.things.getThing("t1", { signal: controller.signal }),
      (error) => error.name === "AbortError"
    );
    assert.equal(calls.length, 1);
  });

  it("times out each attempt", async () => {
    const hang = (init) =>
      new Promise((_, reject) => init.signal.addEventListener("abort", () => reject(init.signal.reason)));
    const { calls, torture } = client([hang], { numRetries: 1 });
    // Node does not keep the process alive for `AbortSignal.timeout` alone.
    const alive = setInterval(() => {}, 1000);
    await assert.rejects(
      torture.things.getThing("t1", { timeout: 10 }),
      (error) => error instanceof sdk.ApiTimeoutError && error.name === "TimeoutError"
    );
    clearInterval(alive);
    assert.equal(calls.length, 2);
  });

  it("throws the error class of the status, with the body parsed by its declared schema", async () => {
    const body = { message: "invalid", fields: { name: ["required"] } };
    const { torture } = client([json(body, 422, { "x-request-id": "req_1" })]);
    await assert.rejects(torture.things.createThing({ name: "n", kind: "alpha" }), (error) => {
      assert.ok(error instanceof sdk.UnprocessableEntityError);
      assert.ok(error instanceof sdk.ApiError && error instanceof sdk.ApiException);
      assert.equal(error.name, "UnprocessableEntityError");
      assert.equal(error.status, 422);
      assert.deepEqual(error.error, body);
      assert.equal(error.body, JSON.stringify(body));
      assert.equal(error.requestId, "req_1");
      return true;
    });
  });

  it("keeps undeclared and unparsable error bodies as text", async () => {
    const { torture } = client([new Response("gone", { status: 404 })]);
    await assert.rejects(torture.things.getThing("t1"), (error) => {
      assert.ok(error instanceof sdk.NotFoundError);
      assert.equal(error.error, undefined);
      assert.equal(error.body, "gone");
      return true;
    });
    const { torture: other } = client([json({}, 418), json({}, 503)], { numRetries: 0 });
    await assert.rejects(other.things.getThing("t1"), (error) => error.constructor === sdk.ApiError);
    await assert.rejects(other.things.getThing("t1"), sdk.InternalServerError);
    assert.equal(sdk.Torture.NotFoundError, sdk.NotFoundError);
  });

  it("times out after the default timeout unless disabled", async () => {
    const { calls, torture } = client([json(THING)]);
    await torture.things.getThing("t1");
    assert.ok(calls[0].init.signal instanceof AbortSignal);
    const { calls: unbounded, torture: patient } = client([json(THING)], { requestTimeout: Infinity });
    await patient.things.getThing("t1");
    assert.equal(unbounded[0].init.signal, undefined);
  });
});

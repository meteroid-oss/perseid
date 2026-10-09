// Generates tests/fixtures/features.yaml (default `int64 = "number"`) and drives the client
// against a fake fetch: what goes on the wire (paths, queries, headers, cookies, keys) and how
// failures surface (typed errors, retries, timeouts, cancellation, decoding).
import assert from "node:assert/strict";
import { after, describe, it } from "node:test";
import { generate } from "./_sdk.mjs";

const { sdk, cleanup } = await generate("features.yaml", { name: "Features", idempotencyKeys: true });
after(cleanup);

const json = (body, status = 200, headers = {}) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", "x-request-id": "req_1", ...headers },
  });
const text = (body, status = 200, headers = {}) => new Response(body, { status, headers });
const OK = { status: "ok" };

/** A client whose fetch answers with `responses`, the last one repeating; `calls` records each attempt. */
function client(responses, options = {}) {
  const calls = [];
  const fetch = async (url, init) => {
    calls.push({
      url: new URL(url),
      method: init.method,
      headers: { ...init.headers },
      body: init.body,
    });
    const next = responses[Math.min(calls.length - 1, responses.length - 1)];
    return typeof next === "function" ? next(init, calls.length) : next.clone();
  };
  return { calls, features: new sdk.Features({ apiKey: "tok", fetch, maxRetries: 0, ...options }) };
}

/** A fetch that never answers, until its request is aborted. */
const hang = (init) =>
  new Promise((_, reject) => {
    const abort = () => reject(init.signal.reason);
    if (init.signal.aborted) {
      abort();
    } else {
      init.signal.addEventListener("abort", abort, { once: true });
    }
  });

/** The error a call fails with, whether it throws or rejects. */
async function failure(call) {
  try {
    await call();
  } catch (error) {
    return error;
  }
  return assert.fail("expected the call to fail");
}

describe("path parameters", () => {
  const ENCODED = {
    plain: "plain",
    "sp ace": "sp%20ace",
    "sl/ash": "sl%2Fash",
    "q?mark": "q%3Fmark",
    "per%cent": "per%25cent",
    "ha#sh": "ha%23sh",
    "lit%25eral": "lit%2525eral",
    "a+b": "a%2Bb",
    "héllo wörld ✓": "h%C3%A9llo%20w%C3%B6rld%20%E2%9C%93",
    "\u{1F600}": "%F0%9F%98%80",
    "..%2F..": "..%252F..",
    "a/../b": "a%2F..%2Fb",
    "%2e%2e": "%252e%252e",
    "back\\slash": "back%5Cslash",
    "tab\there": "tab%09here",
    "a;b,c=d&e": "a%3Bb%2Cc%3Dd%26e",
  };

  for (const [value, encoded] of Object.entries(ENCODED)) {
    it(`escapes ${JSON.stringify(value)}`, async () => {
      const { calls, features } = client([json(OK)]);
      await features.encoding.retrieveScenarioPath(value);
      const { url } = calls[0];
      assert.equal(url.pathname, `/v1/scenarios/paths/${encoded}`);
      assert.equal(url.search, "");
      assert.equal(url.hash, "");
    });
  }

  it("refuses the dot segments a URL would resolve away", async () => {
    for (const value of [".", ".."]) {
      const { calls, features } = client([json(OK)]);
      const error = await failure(() => features.encoding.retrieveScenarioPath(value));
      assert.ok(error instanceof sdk.FeaturesError, `${value}: ${error}`);
      assert.equal(calls.length, 0);
    }
  });

  it("sends integers as their digits", async () => {
    const { calls, features } = client([json(OK)]);
    await features.errors.retrieveScenarioStatus(404);
    assert.equal(calls[0].url.pathname, "/v1/scenarios/status/404");
  });
});

describe("query parameters, headers and cookies", () => {
  it("encodes query text so that the server reads it back", async () => {
    const values = ["plain", "sp ace", "a&b=c+d", "100%", "slash/qm?", "héllo wörld ✓"];
    for (const value of values) {
      const { calls, features } = client([json(OK)]);
      await features.encoding.retrieveScenariosQuery({ q: value });
      assert.equal(new URLSearchParams(calls[0].url.search).get("q"), value);
      assert.equal(calls[0].url.search.includes("?", 1), false);
    }
    const { calls, features } = client([json(OK)]);
    await features.encoding.retrieveScenariosQuery({ q: "a&b=c+d" });
    assert.equal(calls[0].url.search, "?q=a%26b%3Dc%2Bd");
  });

  it("repeats exploded arrays in order and keeps booleans as words", async () => {
    const { calls, features } = client([json(OK)]);
    await features.encoding.retrieveScenariosMulti({ ids: ["b", "a", "c"], flag: true });
    assert.equal(calls[0].url.search, "?ids=b&ids=a&ids=c&flag=true");
  });

  it("sends the required header, and the optional one only when given", async () => {
    const { calls, features } = client([json(OK)]);
    await features.encoding.listScenariosHeaders({ xTenant: "acme" });
    await features.encoding.listScenariosHeaders({ xTenant: "acme", xTraceId: "t1" });
    assert.equal(calls[0].headers["x-tenant"], "acme");
    assert.equal("x-trace-id" in calls[0].headers, false);
    assert.equal(calls[1].headers["x-tenant"], "acme");
    assert.equal(calls[1].headers["x-trace-id"], "t1");
  });

  it("sends dates and datetimes as RFC 3339 text", async () => {
    const { calls, features } = client([json({ at: "2024-01-02T03:04:05.250Z", day: "2024-01-02" })]);
    const at = new Date("2024-01-02T03:04:05.250Z");
    const box = await features.encoding.retrieveScenariosDatetime({ since: at, day: "2024-01-02" });
    assert.equal(new URLSearchParams(calls[0].url.search).get("since"), "2024-01-02T03:04:05.250Z");
    assert.equal(box.at.getTime(), at.getTime());
    await features.encoding.scenariosDatetime({ at, day: "2024-01-02" });
    assert.deepEqual(JSON.parse(calls[1].body), { at: "2024-01-02T03:04:05.250Z", day: "2024-01-02" });
  });

  it("percent-encodes cookie values and sends the API key cookie", async () => {
    const { calls, features } = client([json(OK)]);
    await features.cookies.retrieveScenariosCookie({ sessionId: "a b;c" });
    assert.equal(calls[0].headers.cookie, "session_id=a%20b%3Bc");
    assert.equal("authorization" in calls[0].headers, false);

    const keyed = client([json(OK)], { apiKeys: { apiKeyCookie: "ck1" } });
    await keyed.features.cookies.retrieveScenariosCookieAuth();
    assert.equal(keyed.calls[0].headers.cookie, "auth_token=ck1");
    assert.equal("authorization" in keyed.calls[0].headers, false);
  });

  it("sends no credentials to an operation that declares none", async () => {
    const { calls, features } = client([json({ id: "i1", name: "n" })]);
    await features.items.retrieve("i1");
    for (const name of ["authorization", "x-api-key", "cookie"]) {
      assert.equal(name in calls[0].headers, false, name);
    }
    assert.equal(calls[0].url.search, "");
  });
});

describe("base URL", () => {
  it("keeps the path prefix, with or without a trailing slash", async () => {
    for (const baseURL of ["https://x.test/api/v2", "https://x.test/api/v2/", "https://x.test/api/v2///"]) {
      const { calls, features } = client([json({ id: "i1", name: "n" })], { baseURL });
      await features.items.retrieve("i1");
      assert.equal(calls[0].url.href, "https://x.test/api/v2/items/i1");
    }
  });

  it("keeps the query of the operation path next to the parameters", async () => {
    const { calls, features } = client([json(OK)], { baseURL: "https://x.test/p" });
    await features.wire.betaSearch({ limit: 2, features: ["x", "y"] });
    assert.equal(calls[0].url.href, "https://x.test/p/wire/beta?beta=true&limit=2");
    assert.equal(calls[0].headers.features, "x,y");
  });

  it("adds default and per-call query parameters", async () => {
    const { calls, features } = client([json({ id: "i1", name: "n" })], {
      baseURL: "https://x.test/p",
      defaultQuery: { v: "1" },
    });
    await features.items.retrieve("i1", { query: { debug: "yes" } });
    assert.equal(calls[0].url.search, "?v=1&debug=yes");
  });
});

describe("errors", () => {
  it("raises the typed authentication error for an unauthenticated call", async () => {
    delete process.env.FEATURES_API_KEY;
    const calls = [];
    const fetch = async (url, init) => {
      calls.push({ headers: { ...init.headers } });
      return json({ error: "unauthorized" }, 401);
    };
    const anonymous = new sdk.Features({ fetch, maxRetries: 0 });
    const error = await failure(() => anonymous.widgets.list());
    assert.ok(error instanceof sdk.AuthenticationError && error instanceof sdk.APIError);
    assert.equal(error.status, 401);
    assert.deepEqual({ ...error.error }, { error: "unauthorized", code: undefined });
    assert.equal(error.requestId, "req_1");
    assert.equal("authorization" in calls[0].headers, false);
    assert.equal("x-api-key" in calls[0].headers, false);
  });

  it("carries the request id of every status, whatever the body", async () => {
    for (const status of [400, 401, 403, 404, 409, 422, 429, 500, 503]) {
      const { features } = client([json({ error: "e", code: status }, status, { "x-request-id": `req_${status}` })]);
      const error = await failure(() => features.errors.retrieveScenarioStatus(status));
      assert.ok(error instanceof sdk.APIError);
      assert.equal(error.status, status);
      assert.equal(error.requestId, `req_${status}`);
      assert.equal(error.error.error, "e");
      assert.equal(error.error.code, status);
    }
    const { features } = client([text("", 404, { "request-id": "req_alt" })]);
    const error = await failure(() => features.items.retrieve("i1"));
    assert.equal(error.requestId, "req_alt");
  });

  it("keeps a body that is not JSON, and has no parsed error", async () => {
    const html = "<html><body>Bad gateway</body></html>";
    const { features } = client([text(html, 502, { "content-type": "text/html", "x-request-id": "req_9" })]);
    const error = await failure(() => features.items.retrieve("i1"));
    assert.ok(error instanceof sdk.InternalServerError);
    assert.equal(error.status, 502);
    assert.equal(error.body, html);
    assert.equal(error.error, undefined);
    assert.equal(error.requestId, "req_9");
    assert.ok(error.message.includes("502"));
  });

  it("handles an empty error body", async () => {
    const { features } = client([new Response(null, { status: 404, headers: { "x-request-id": "req_e" } })]);
    const error = await failure(() => features.items.retrieve("i1"));
    assert.ok(error instanceof sdk.NotFoundError);
    assert.equal(error.body, "");
    assert.equal(error.error, undefined);
    assert.equal(error.requestId, "req_e");
  });

  it("keeps JSON error bodies of another shape", async () => {
    const { features } = client([json({ message: "nope", details: [1, 2] }, 400)]);
    const error = await failure(() => features.items.retrieve("i1"));
    assert.ok(error instanceof sdk.BadRequestError);
    assert.equal(error.error.message, "nope");
    assert.deepEqual(error.error.details, [1, 2]);
    assert.deepEqual(JSON.parse(error.body), { message: "nope", details: [1, 2] });

    const nothing = client([text("null", 422, { "content-type": "application/json" })]);
    const empty = await failure(() => nothing.features.items.retrieve("i1"));
    assert.ok(empty instanceof sdk.UnprocessableEntityError);
    assert.equal(empty.body, "null");
  });

  it("raises an error from the first page of a paginated call", async () => {
    const server = client([json({ error: "boom", code: 500 }, 500, { "x-request-id": "req_p" })]);
    const awaited = await failure(() => server.features.widgets.list());
    assert.ok(awaited instanceof sdk.InternalServerError);
    assert.equal(awaited.requestId, "req_p");

    const items = [];
    const iterated = await failure(async () => {
      for await (const widget of server.features.widgets.list()) {
        items.push(widget);
      }
    });
    assert.ok(iterated instanceof sdk.InternalServerError);
    assert.deepEqual(items, []);
    assert.equal(server.calls.length, 2);

    const gone = client([json({ error: "page_gone", code: 409 }, 409)]);
    const conflict = await failure(() => gone.features.errors.listScenariosPages());
    assert.ok(conflict instanceof sdk.ConflictError);
    assert.equal(conflict.error.error, "page_gone");
  });

  it("raises the error of a later page after the items before it, without looping", async () => {
    const first = json({ data: [{ id: "p1", name: "p1" }, { id: "p2", name: "p2" }], next_cursor: "c2" });
    const { calls, features } = client([first, json({ error: "page_gone", code: 409 }, 409)]);
    const ids = [];
    const error = await failure(async () => {
      for await (const widget of features.errors.listScenariosPages()) {
        ids.push(widget.id);
      }
    });
    assert.deepEqual(ids, ["p1", "p2"]);
    assert.ok(error instanceof sdk.ConflictError);
    assert.equal(error.error.code, 409);
    assert.equal(calls.length, 2);
    assert.equal(calls[1].url.search, "?cursor=c2");
  });
});

describe("responses that cannot be decoded", () => {
  it("raise a decode error for a 2xx body that is not the declared JSON", async () => {
    const bodies = ["", "{", '{"status": ', "<html>ok</html>", "not json", '{"status": "a"} trailing'];
    for (const body of bodies) {
      const { features } = client([text(body, 200, { "content-type": "application/json" })]);
      const error = await failure(() => features.content.retrieveScenariosMalformed());
      assert.ok(error instanceof sdk.APIDecodeError && error instanceof sdk.FeaturesError, `${JSON.stringify(body)}: ${error}`);
      assert.equal(error.body, body);
    }
  });

  it("raise a decode error for a 201 with garbage, but not for a 204", async () => {
    const created = client([text("{", 201)]);
    await assert.rejects(created.features.content.retrieveScenariosMalformed(), sdk.APIDecodeError);
    const empty = client([new Response(null, { status: 204 })]);
    assert.equal(await empty.features.content.retrieveScenariosEmptyBody(), undefined);
  });

  it("give nothing for a null or empty body of an optional result", async () => {
    for (const body of [text("null"), text(""), new Response(null, { status: 204 })]) {
      const { features } = client([body]);
      assert.equal(await features.content.retrieveScenariosNullableBody(), undefined);
    }
  });

  it("raise a decode error for an event stream of another media type, and for bad event data", async () => {
    const wrong = client([json(OK)]);
    await assert.rejects(wrong.features.streaming.retrieveScenariosSse(), sdk.APIDecodeError);

    const bad = text("data: {oops\n\n", 200, { "content-type": "text/event-stream" });
    const { features } = client([bad]);
    const stream = await features.streaming.createCompletionStream({ prompt: "x" });
    const error = await failure(async () => {
      for await (const chunk of stream) {
        assert.fail(`no chunk expected, got ${JSON.stringify(chunk)}`);
      }
    });
    assert.ok(error instanceof sdk.APIDecodeError);
    assert.equal(error.body, "{oops");
  });

  it("tolerate fields the schema does not declare, at any depth, and send them back", async () => {
    const body = { status: "ok", extra: 1, nested: { a: [1, 2, { b: null }] }, list: [1, "x"] };
    const { features } = client([json(body)]);
    const health = await features.content.listScenariosExtraFields();
    assert.equal(health.status, "ok");
    assert.deepEqual(health.nested, body.nested);
    assert.deepEqual(health.list, [1, "x"]);
    assert.deepEqual(sdk.HealthSerializer.serialize(health), body);

    const page = client([json({ data: [{ id: "w1", name: "n", color: "red", more: { deep: [null] } }], extra: true, next_cursor: null })]);
    const first = await page.features.widgets.list();
    assert.equal(first.items[0].color, "red");
    assert.deepEqual(first.items[0].more, { deep: [null] });
    assert.equal(first.extra, true);
    assert.equal(first.nextCursor, null);
    assert.equal(first.data, first.body.data);
  });
});

describe("retries", () => {
  const FAST = { maxRetries: 2, retryScheduleInMs: [1, 1] };

  it("retries an attempt that timed out, and succeeds", async () => {
    const alive = setInterval(() => {}, 1000);
    try {
      const { calls, features } = client([hang, json(OK)], { ...FAST, timeout: 20 });
      assert.equal((await features.content.retrieveScenariosMalformed()).status, "ok");
      assert.equal(calls.length, 2);
    } finally {
      clearInterval(alive);
    }
  });

  it("raises the timeout error once the retries are used up", async () => {
    const alive = setInterval(() => {}, 1000);
    try {
      const { calls, features } = client([hang], { ...FAST, timeout: 20 });
      const error = await failure(() => features.items.retrieve("i1"));
      assert.ok(error instanceof sdk.APIConnectionTimeoutError && error instanceof sdk.APIConnectionError);
      assert.equal(calls.length, 3);
    } finally {
      clearInterval(alive);
    }
  });

  it("retries connection failures and 5xx, not 4xx", async () => {
    const refused = () => Promise.reject(new TypeError("fetch failed"));
    const flaky = client([refused, json({ error: "x" }, 503), json({ id: "i1", name: "n" })], FAST);
    assert.equal((await flaky.features.items.retrieve("i1")).id, "i1");
    assert.equal(flaky.calls.length, 3);

    const denied = client([json({ error: "no" }, 403), json({ id: "i1", name: "n" })], FAST);
    await assert.rejects(denied.features.items.retrieve("i1"), sdk.PermissionDeniedError);
    assert.equal(denied.calls.length, 1);
  });

  it("sends the same idempotency key on every attempt", async () => {
    const { calls, features } = client([json({ error: "x" }, 503), json({ error: "x" }, 503), json(OK)], FAST);
    await features.retries.scenariosIdempotent({ amount: 5 }, { xScenarioId: "s", idempotencyKey: "idem-1" });
    assert.equal(calls.length, 3);
    assert.deepEqual(calls.map((call) => call.headers["idempotency-key"]), ["idem-1", "idem-1", "idem-1"]);
    assert.deepEqual(calls.map((call) => call.body), Array(3).fill('{"amount":5}'));

    const given = client([json({ error: "x" }, 503), json(OK)], FAST);
    await given.features.retries.scenariosIdempotent(
      { amount: 5 },
      { xScenarioId: "s", idempotencyKey: "idem-1" },
      { idempotencyKey: "per-call" }
    );
    assert.deepEqual(given.calls.map((call) => call.headers["idempotency-key"]), ["per-call", "per-call"]);
  });

  it("generates one idempotency key per POST call and reuses it across retries", async () => {
    const { calls, features } = client([json({ error: "x" }, 503), json({ text: "X" })], FAST);
    await features.streaming.createCompletion({ prompt: "x" });
    assert.equal(calls.length, 2);
    const [first, second] = calls.map((call) => call.headers["idempotency-key"]);
    assert.ok(first.startsWith("auto_"), first);
    assert.equal(first, second);
    await features.streaming.createCompletion({ prompt: "x" });
    assert.notEqual(calls[2].headers["idempotency-key"], first);

    const read = client([json(OK)]);
    await read.features.encoding.retrieveScenariosMulti({ ids: ["a"], flag: false });
    assert.equal("idempotency-key" in read.calls[0].headers, false);
  });

  it("numbers the retries in a header", async () => {
    const { calls, features } = client([json({ error: "x" }, 503), json({ id: "i1", name: "n" })], FAST);
    await features.items.retrieve("i1");
    assert.equal("features-retry-count" in calls[0].headers, false);
    assert.equal(calls[1].headers["features-retry-count"], "1");
  });

  it("waits for Retry-After, in seconds or as an HTTP date", async () => {
    const retryAfter = (value) =>
      client([json({ error: "x" }, 429, { "retry-after": value }), json({ id: "i1", name: "n" })], { maxRetries: 1 });
    let started = Date.now();
    await retryAfter("1").features.items.retrieve("i1");
    assert.ok(Date.now() - started >= 900);

    started = Date.now();
    await retryAfter(new Date(Date.now() + 2000).toUTCString()).features.items.retrieve("i1");
    assert.ok(Date.now() - started >= 900);
  });
});

describe("cancellation", () => {
  it("aborts an attempt in flight, with the signal's reason as the cause", async () => {
    const { calls, features } = client([hang], { maxRetries: 2 });
    const controller = new AbortController();
    setTimeout(() => controller.abort(new Error("stop")), 10);
    const error = await failure(() => features.items.retrieve("i1", { signal: controller.signal }));
    assert.ok(error instanceof sdk.APIUserAbortError && error instanceof sdk.FeaturesError);
    assert.equal(error.cause.message, "stop");
    assert.equal(calls.length, 1, "an aborted call is not retried");
  });

  it("aborts the wait before a retry", async () => {
    const { calls, features } = client([json({ error: "x" }, 503)], { maxRetries: 2, retryScheduleInMs: [5000] });
    const controller = new AbortController();
    setTimeout(() => controller.abort(), 30);
    const started = Date.now();
    const error = await failure(() => features.items.retrieve("i1", { signal: controller.signal }));
    assert.ok(error instanceof sdk.APIUserAbortError);
    assert.ok(Date.now() - started < 3000, "the wait was cut short");
    assert.equal(calls.length, 1);
  });

  it("does not send a request whose signal is already aborted", async () => {
    const { calls, features } = client([json(OK)]);
    const controller = new AbortController();
    controller.abort();
    await assert.rejects(features.items.retrieve("i1", { signal: controller.signal }), sdk.APIUserAbortError);
    assert.equal(calls.length, 0);
  });
});

describe("int64 values in the default number mode", () => {
  it("are numbers, sent as integer literals", async () => {
    const { calls, features } = client([text('{"value":9007199254740993,"min":-9223372036854775808}')]);
    const big = await features.encoding.scenariosBigint({ value: 42, min: -7 });
    assert.deepEqual(JSON.parse(calls[0].body), { min: -7, value: 42 });
    assert.equal(calls[0].body.includes("."), false);
    assert.equal(typeof big.value, "number");
    assert.equal(typeof big.min, "number");
  });

  it("round-trip exactly up to 2^53", async () => {
    const safe = Number.MAX_SAFE_INTEGER;
    const { features } = client([text(`{"value":${safe},"min":${-safe}}`)]);
    const big = await features.encoding.scenariosBigint({ value: safe, min: -safe });
    assert.equal(big.value, safe);
    assert.equal(big.min, -safe);
  });

  it("read the offset-paginated int64 total of a page", async () => {
    const { features } = client([json({ data: [{ id: "r1" }, { id: "r2" }], total: 3 }), json({ data: [{ id: "r3" }], total: 3 })], {
      apiKeys: { apiKeyQuery: "tok" },
    });
    const ids = [];
    for await (const entry of features.records.list()) {
      ids.push(entry.id);
    }
    assert.deepEqual(ids, ["r1", "r2", "r3"]);
    const page = await features.records.list();
    assert.equal(page.total, 3);
  });
});

describe("binary responses", () => {
  /** A response streaming `chunks`, counting those the server produced; then closing, failing or hanging. */
  function chunked(chunks, { end = "close", headers = {} } = {}) {
    const state = { sent: 0, cancelled: false };
    const body = new ReadableStream({
      pull(controller) {
        if (state.sent < chunks.length) {
          controller.enqueue(new TextEncoder().encode(chunks[state.sent++]));
        } else if (end === "close") {
          controller.close();
        } else if (end === "fail") {
          controller.error(new TypeError("terminated"));
        } else {
          return new Promise(() => {});
        }
      },
      cancel() {
        state.cancelled = true;
      },
    });
    return { state, response: new Response(body, { status: 200, headers }) };
  }

  it("reads the chunks as they are consumed", async () => {
    const { state, response } = chunked(["ab", "cd", "ef"], { headers: { "content-type": "image/png" } });
    const { features } = client([() => response]);
    const download = await features.content.downloadImage();
    assert.ok(download instanceof sdk.BinaryResponse);
    assert.equal(download.status, 200);
    assert.equal(download.headers.get("content-type"), "image/png");
    const seen = [];
    for await (const chunk of download) {
      seen.push(new TextDecoder().decode(chunk));
      assert.ok(state.sent <= seen.length + 1, "the body is read no further ahead than the next chunk");
    }
    assert.deepEqual(seen, ["ab", "cd", "ef"]);
  });

  it("reads the whole body as bytes, a buffer, a blob or text", async () => {
    const { features } = client([() => chunked(["ab", "cd"], { headers: { "content-type": "image/png" } }).response]);
    assert.deepEqual(Array.from(await (await features.content.downloadBlob()).bytes()), [97, 98, 99, 100]);
    assert.equal((await (await features.content.downloadBlob()).arrayBuffer()).byteLength, 4);
    const blob = await (await features.content.downloadBlob()).blob();
    assert.equal(blob.type, "image/png");
    assert.equal(await blob.text(), "abcd");
    assert.equal(await (await features.content.downloadBlob()).text(), "abcd");
  });

  it("streams to a file with Node's writeFile", async () => {
    const { mkdtemp, readFile, rm, writeFile } = await import("node:fs/promises");
    const { join } = await import("node:path");
    const { tmpdir } = await import("node:os");
    const directory = await mkdtemp(join(tmpdir(), "binary-"));
    try {
      const { features } = client([() => chunked(["ab", "cd"]).response]);
      await writeFile(join(directory, "blob.bin"), await features.content.downloadBlob());
      assert.equal(await readFile(join(directory, "blob.bin"), "utf8"), "abcd");
    } finally {
      await rm(directory, { recursive: true });
    }
  });

  it("raises an error status before giving any body", async () => {
    const { features } = client([json({ error: "gone" }, 404)]);
    const error = await failure(() => features.content.downloadBlob());
    assert.ok(error instanceof sdk.NotFoundError);
    assert.deepEqual(JSON.parse(error.body), { error: "gone" });
  });

  it("retries until the headers arrive, not once the body is handed over", async () => {
    const { calls, features } = client([json({ error: "x" }, 503), () => chunked(["ab"], { end: "fail" }).response], {
      maxRetries: 2,
      retryScheduleInMs: [1, 1],
    });
    const download = await features.content.downloadBlob();
    assert.equal(calls.length, 2);
    const error = await failure(() => download.bytes());
    assert.ok(error instanceof sdk.APIConnectionError, String(error));
    assert.equal(calls.length, 2);
  });

  it("times out a read that stalls, not a long download", async () => {
    const slow = () => {
      let sent = 0;
      const body = new ReadableStream({
        async pull(controller) {
          await new Promise((resolve) => setTimeout(resolve, 15));
          if (sent++ < 6) controller.enqueue(new Uint8Array([sent]));
          else controller.close();
        },
      });
      return new Response(body);
    };
    const { features } = client([slow], { timeout: 50 });
    assert.equal((await (await features.content.downloadBlob()).bytes()).length, 6, "longer than the timeout in all");

    const stalled = chunked(["ab"], { end: "hang" });
    const hanging = client([() => stalled.response], { timeout: 30 });
    const download = await hanging.features.content.downloadBlob();
    const error = await failure(() => download.bytes());
    assert.ok(error instanceof sdk.APIConnectionTimeoutError, String(error));
    assert.ok(stalled.state.cancelled, "the stalled body is cancelled");
  });

  it("releases an unread body on cancel", async () => {
    const { state, response } = chunked(["ab", "cd"]);
    const { features } = client([() => response]);
    const download = await features.content.downloadBlob();
    await download.cancel();
    assert.ok(state.cancelled);
  });

  it("gives the unread body with the response", async () => {
    const { features } = client([() => chunked(["ab"], { headers: { "x-request-id": "req_9" } }).response]);
    const { data, requestId } = await features.content.downloadBlob().withResponse();
    assert.equal(requestId, "req_9");
    assert.equal(await data.text(), "ab");
  });
});

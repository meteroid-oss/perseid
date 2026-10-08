import { strict as assert } from "node:assert";
import {
  APIConnectionError,
  APIDecodeError,
  APIError,
  AuthenticationError,
  BadRequestError,
  ConflictError,
  Features,
  FeaturesError,
  InternalServerError,
  NotFoundError,
  PermissionDeniedError,
  type SseEvent,
  UnprocessableEntityError,
  type Widget,
} from "../src";

const baseURL = process.env.FEATURES_URL!;

async function collect<T extends { id: string }>(items: AsyncIterable<T>): Promise<string[]> {
  const ids: string[] = [];
  for await (const item of items) {
    ids.push(item.id);
  }
  return ids;
}

/** The error a call fails with; fails the smoke test when it succeeds or fails otherwise. */
async function failure(call: PromiseLike<unknown>): Promise<APIError> {
  try {
    await call;
  } catch (error) {
    assert.ok(error instanceof APIError, `expected an APIError, got ${String(error)}`);
    return error;
  }
  return assert.fail("expected the call to fail");
}

let counter = 0;
/** A scenario id no other test case uses, scoping the server state of the retry scenarios. */
function scenarioId(scenario: string): string {
  return `typescript-${scenario}-${++counter}`;
}

/** What the server saw of a scenario id: how many attempts, with which idempotency keys. */
async function seen(id: string): Promise<{ id: string; attempts: number; keys: string[] }> {
  const response = await fetch(`${baseURL}/__server/attempts/${id}`);
  return (await response.json()) as { id: string; attempts: number; keys: string[] };
}

/** A `fetch` that counts the requests it sends. */
function counting(): { fetch: typeof fetch; calls: () => number } {
  let calls = 0;
  return {
    fetch: (...args: Parameters<typeof fetch>) => {
      calls++;
      return fetch(...args);
    },
    calls: () => calls,
  };
}

const PATH_VALUES = [
  "plain",
  "sp ace",
  "sl/ash",
  "q?mark",
  "per%cent",
  "ha#sh",
  "lit%25eral",
  "a+b",
  "h\u00e9llo w\u00f6rld \u2713",
];
const QUERY_VALUES = ["plain", "sp ace", "a&b=c+d", "100%", "slash/qm?", "h\u00e9llo w\u00f6rld \u2713"];
const INSTANT = new Date("2024-01-02T03:04:05.250Z");
const BYTES = [0x68, 0x65, 0x6c, 0x6c, 0x6f, 0xfb, 0xff, 0xfe];
const BYTES_BASE64 = "aGVsbG/7//4=";

/** Items: methods and empty responses. */
async function items(client: Features) {
  const item = await client.items.retrieve("i1");
  assert.deepEqual([item.id, item.name, item.note], ["i1", "first", "hi"]);
  const patched = await client.items.update("i1", { name: "renamed" });
  assert.equal(patched.name, "renamed");
  assert.equal(patched.note, null);
  assert.equal(await client.items.delete("i1"), undefined);
}

/** Content and decoding. */
async function content(client: Features) {
  assert.equal(await client.content.retrieveScenariosNullableBody(), undefined);
  assert.equal(await client.content.retrieveScenariosText(), "hello text\n");
  assert.equal(await client.content.retrieveScenariosCsv(), 'id,name\n1,alpha\n2,"be,ta"\n');

  const blob = await client.content.downloadBlob();
  assert.ok(blob instanceof Uint8Array);
  assert.equal(blob.length, 256);
  assert.deepEqual(Array.from(blob), Array.from({ length: 256 }, (_, i) => i));
  const image = await client.content.downloadImage();
  assert.deepEqual(
    Array.from(image),
    [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, ...Array.from({ length: 4 }, () => [0, 1, 0xfe, 0xff]).flat()]
  );

  await assert.rejects(client.content.retrieveScenariosMalformed(), (error: unknown) => {
    assert.ok(error instanceof APIDecodeError && error instanceof FeaturesError);
    assert.equal(error.body, '{"status": ');
    return true;
  });
  await assert.rejects(client.content.retrieveScenariosEmptyBody(), (error: unknown) => {
    assert.ok(error instanceof APIDecodeError);
    assert.equal(error.body, "");
    return true;
  });
  assert.equal((await client.content.listScenariosExtraFields()).status, "ok");

  const nulls = await client.content.listScenariosNulls();
  assert.equal(nulls.name, null);
  assert.deepEqual(nulls.tags, ["a", null, "b"]);
  assert.deepEqual(nulls.counts, { x: 1n, y: null });
  assert.equal(nulls.note, null);
  const echoed = await client.content.createScenariosNull({
    name: null,
    tags: ["a", null],
    counts: { x: null, y: 2n },
  });
  assert.equal(echoed.name, null);
  assert.deepEqual(echoed.tags, ["a", null]);
  assert.deepEqual(echoed.counts, { x: null, y: 2n });
  assert.equal(echoed.note, undefined);

  const bag = await client.content.retrieveScenariosBag();
  const extras = bag as unknown as Record<string, unknown>;
  assert.equal(bag.id, "b1");
  assert.equal(Number(extras.a), 1);
  assert.equal(Number(extras.b), 2);
  assert.deepEqual(await client.content.listScenariosLabels(), { k: "v", z: "y" });

  const known = await client.content.retrieveScenariosEnum({ mode: "known" });
  assert.equal(known.kind, "red");
  assert.deepEqual(known.kinds, ["green", "blue"]);
  const unlisted = await client.content.retrieveScenariosEnum({ mode: "unknown" });
  assert.equal(unlisted.kind as string, "magenta");
  assert.deepEqual(unlisted.kinds as string[], ["red", "magenta"]);
}

/** Encoding. */
async function encoding(client: Features) {
  const big = await client.encoding.scenariosBigint({
    value: 9007199254740993n,
    min: -9223372036854775808n,
  });
  assert.equal(big.value, 9007199254740993n);
  assert.equal(big.min, -9223372036854775808n);

  const sent = { data: BYTES_BASE64 };
  assert.deepEqual(Array.from(Buffer.from((await client.encoding.createScenariosByte(sent)).data, "base64")), BYTES);
  assert.deepEqual(Array.from(Buffer.from((await client.encoding.listScenariosBytes()).data, "base64")), BYTES);

  const queried = await client.encoding.retrieveScenariosDatetime({ since: INSTANT, day: "2024-01-02" });
  assert.equal(queried.at.getTime(), INSTANT.getTime());
  assert.equal(queried.day, "2024-01-02");
  const posted = await client.encoding.scenariosDatetime({ at: INSTANT, day: "2024-01-02" });
  assert.equal(posted.at.getTime(), INSTANT.getTime());
  assert.equal(posted.day, "2024-01-02");

  for (const value of PATH_VALUES) {
    assert.equal((await client.encoding.retrieveScenarioPath(value)).status, value, value);
  }
  for (const value of QUERY_VALUES) {
    assert.equal((await client.encoding.retrieveScenariosQuery({ q: value })).status, value, value);
  }
  assert.equal((await client.encoding.retrieveScenariosMulti({ ids: ["b", "a", "c"], flag: true })).status, "b,a,c");

  assert.equal((await client.encoding.listScenariosHeaders({ xTenant: "acme" })).status, "acme|");
  assert.equal((await client.encoding.listScenariosHeaders({ xTenant: "acme", xTraceId: "t1" })).status, "acme|t1");
}

/** Cookies. */
async function cookies(client: Features, baseURL: string) {
  assert.equal((await client.cookies.retrieveScenariosCookie({ sessionId: "abc123" })).status, "abc123");
  const keyed = new Features({ baseURL, apiKeys: { apiKeyCookie: "ck1" } });
  assert.equal((await keyed.cookies.retrieveScenariosCookieAuth()).status, "ck1");
  const anonymous = await failure(new Features({ baseURL }).cookies.retrieveScenariosCookieAuth());
  assert.ok(anonymous instanceof AuthenticationError);
}

/** OAuth2 client credentials. */
async function oauth(baseURL: string) {
  const secret = "p@ss word";
  const cached = new Features({ baseURL, clientId: "typescript-oauth", clientSecret: secret });
  assert.equal((await cached.account.retrieveMachine()).status, "Bearer at-typescript-oauth-1||");
  assert.equal((await cached.account.retrieveMachine()).status, "Bearer at-typescript-oauth-1||");
  assert.equal((await seen("typescript-oauth")).attempts, 1, "the token is cached");

  const inBody = new Features({ baseURL, clientId: "typescript-oauth-body", clientSecret: secret, oauthClientAuth: "body" });
  assert.equal((await inBody.account.retrieveMachine()).status, "Bearer at-typescript-oauth-body-1||");

  const revoked = new Features({ baseURL, clientId: "typescript-oauth-revoked", clientSecret: secret, maxRetries: 0 });
  assert.equal((await revoked.account.retrieveMachine()).status, "Bearer at-typescript-oauth-revoked-2||");
  assert.equal((await seen("typescript-oauth-revoked")).attempts, 2, "a rejected token is replaced");

  const concurrent = new Features({ baseURL, clientId: "typescript-oauth-concurrent", clientSecret: secret });
  const both = await Promise.all([concurrent.account.retrieveMachine(), concurrent.account.retrieveMachine()]);
  assert.deepEqual(
    both.map((machine) => machine.status),
    ["Bearer at-typescript-oauth-concurrent-1||", "Bearer at-typescript-oauth-concurrent-1||"]
  );
  assert.equal((await seen("typescript-oauth-concurrent")).attempts, 1, "concurrent calls share one token request");

  const invalid = new Features({ baseURL, clientId: "typescript-oauth-bad", clientSecret: "wrong", maxRetries: 0 });
  const error = await failure(invalid.account.retrieveMachine());
  assert.ok(error instanceof AuthenticationError);
  assert.equal((error.error as { error?: string } | undefined)?.error, "invalid_client");

  const withToken = new Features({ baseURL, apiKey: "tok", clientId: "typescript-oauth", clientSecret: secret });
  assert.equal((await withToken.account.retrieveMachine()).status, "Bearer tok||");
}

/** Retries and idempotency. */
async function retries(client: Features) {
  let id = scenarioId("flaky");
  let started = Date.now();
  assert.equal((await client.retries.retrieveScenariosFlaky({ xScenarioId: id })).status, "attempt=2");
  assert.ok(Date.now() - started >= 900, "the first retry waits for Retry-After: 1");
  assert.equal((await seen(id)).attempts, 2);

  id = scenarioId("flaky-no-retries");
  let error = await failure(client.retries.retrieveScenariosFlaky({ xScenarioId: id }, { maxRetries: 0 }));
  assert.ok(error instanceof InternalServerError);
  assert.equal(error.status, 503);
  assert.equal(error.error?.error, "unavailable");
  assert.equal((await seen(id)).attempts, 1);

  id = scenarioId("rate-limited");
  started = Date.now();
  assert.equal((await client.retries.retrieveScenariosRateLimited({ xScenarioId: id })).status, "attempt=2");
  assert.ok(Date.now() - started >= 900, "the HTTP-date Retry-After is honoured");
  assert.equal((await seen(id)).attempts, 2);

  id = scenarioId("always-unavailable");
  error = await failure(client.retries.retrieveScenariosUnavailable({ xScenarioId: id }, { maxRetries: 2 }));
  assert.equal(error.status, 503);
  assert.equal((await seen(id)).attempts, 3);

  id = scenarioId("idempotent");
  const created = await client.retries.scenariosIdempotent(
    { amount: 5 },
    { xScenarioId: id, idempotencyKey: "idem-1" }
  );
  assert.equal(created.status, "attempts=2;key=idem-1");
  assert.deepEqual((await seen(id)).keys, ["idem-1", "idem-1"]);
}

/** Errors and pagination. */
async function errors(client: Features, baseURL: string) {
  const classes = new Map<number, typeof APIError>([
    [400, BadRequestError],
    [401, AuthenticationError],
    [403, PermissionDeniedError],
    [404, NotFoundError],
    [409, ConflictError],
    [422, UnprocessableEntityError],
  ]);
  for (const [code, errorClass] of classes) {
    const counted = counting();
    const once = new Features({ apiKey: "tok", baseURL, fetch: counted.fetch });
    const error = await failure(once.errors.retrieveScenarioStatus(code));
    assert.ok(error instanceof errorClass, `status ${code} is a ${errorClass.name}`);
    assert.equal(error.status, code);
    assert.equal(error.error?.error, `status ${code}`);
    assert.equal(error.error?.code, code);
    assert.deepEqual(JSON.parse(error.body), { error: `status ${code}`, code });
    assert.equal(error.requestId, "req_mock");
    assert.equal(counted.calls(), 1, `status ${code} is not retried`);
  }

  const counted = counting();
  const paged = new Features({ apiKey: "tok", baseURL, fetch: counted.fetch });
  const ids: string[] = [];
  const gone = await failure(
    (async () => {
      for await (const widget of paged.errors.listScenariosPages()) {
        ids.push(widget.id);
      }
    })()
  );
  assert.deepEqual(ids, ["p1", "p2"]);
  assert.ok(gone instanceof ConflictError);
  assert.equal(gone.status, 409);
  assert.equal(gone.error?.error, "page_gone");
  assert.equal(gone.error?.code, 409);
  assert.equal(counted.calls(), 2, "the failed page is not fetched again");
}

/** Streaming. */
async function streaming(client: Features) {
  const events: SseEvent[] = [];
  for await (const event of await client.streaming.retrieveScenariosSse()) {
    events.push(event);
  }
  assert.deepEqual(events, [
    { event: "message", data: "first", id: undefined, retry: undefined },
    { event: "tick", data: "line1\nline2", id: "7", retry: undefined },
    { event: "message", data: '{"n": 3}', id: "7", retry: 2500 },
    { event: "message", data: "tail", id: "7", retry: undefined },
  ]);

  const denied = await failure(client.streaming.retrieveScenariosSseError());
  assert.ok(denied instanceof PermissionDeniedError);
  assert.equal(denied.status, 403);
  assert.equal(denied.error?.error, "forbidden");
  assert.equal(denied.error?.code, 403);
}

async function main() {
  const client = new Features({ apiKey: "tok", baseURL });
  assert.equal((await client.account.checkHealth()).status, "||");
  assert.equal((await client.account.retrieveMachine()).status, "Bearer tok||");
  assert.deepEqual(await collect(client.widgets.list()), ["w1", "w2", "w3"]);
  assert.deepEqual(
    await collect(client.widgets.listEvents("w1", { kind: "created" })),
    ["e1", "e2", "e3"]
  );
  // From `ending_before`, pages go backwards and never send `starting_after`.
  assert.deepEqual(
    await collect(client.widgets.listEvents("w1", { kind: "created", endingBefore: "e9" })),
    ["e7", "e8", "e6"]
  );
  assert.deepEqual(await collect(client.gadgets.list()), ["g1", "g2", "g3"]);
  assert.deepEqual(await collect(client.records.list()), ["r1", "r2", "r3"]);

  const basic = new Features({ baseURL, basicAuth: { username: "u", password: "p" } });
  assert.equal((await basic.account.createSession()).status, "Basic dTpw||");

  const provided = new Features({ baseURL, tokenProvider: async () => "fresh" });
  assert.equal((await provided.account.retrieveMachine()).status, "Bearer fresh||");

  const keyed = new Features({ baseURL, apiKeys: { apiKey: "k" } });
  assert.deepEqual(await collect(keyed.widgets.list()), ["w1", "w2", "w3"]);
  await assert.rejects(new Features({ baseURL }).widgets.list(), (error: unknown) => {
    assert.ok(error instanceof AuthenticationError && error instanceof FeaturesError);
    assert.equal(error.error?.error, "unauthorized");
    assert.equal(error.error?.code, undefined);
    assert.equal(error.requestId, "req_mock");
    return true;
  });
  await assert.rejects(
    new Features({ apiKey: "tok", baseURL: "http://127.0.0.1:9", maxRetries: 0 }).widgets.list(),
    APIConnectionError
  );

  process.env.FEATURES_API_KEY = "env";
  process.env.FEATURES_BASE_URL = baseURL;
  assert.equal((await new Features().account.retrieveMachine()).status, "Bearer env||");
  assert.equal((await new Features({ apiKey: "arg" }).account.retrieveMachine()).status, "Bearer arg||");
  delete process.env.FEATURES_API_KEY;
  delete process.env.FEATURES_BASE_URL;

  const first = await client.widgets.list();
  assert.deepEqual(first.items.map((widget: Widget) => widget.id), ["w1", "w2"]);
  assert.deepEqual(first.data.map((widget: Widget) => widget.id), ["w1", "w2"]);
  assert.equal(first.nextCursor, "c2");
  assert.equal((first.items[0] as Widget & { color?: string }).color, "red");
  assert.equal(first.response.status, 200);
  assert.deepEqual(JSON.parse(JSON.stringify(first)), JSON.parse(JSON.stringify(first.body)));
  assert.ok(first.hasNextPage());
  const second = await first.getNextPage();
  assert.deepEqual(second.items.map((widget: Widget) => widget.id), ["w3"]);
  assert.equal(second.nextCursor, null);
  assert.equal(second.body.nextCursor, null);
  assert.ok(!second.hasNextPage());
  const pageIds = [];
  for await (const page of first.iterPages()) {
    pageIds.push(page.items.length);
    assert.equal(page.data.length, page.items.length);
  }
  assert.deepEqual(pageIds, [2, 1]);
  assert.deepEqual(await collect(first), ["w1", "w2", "w3"]);
  assert.deepEqual(await collect(client.widgets.list()), ["w1", "w2", "w3"]);

  const gadgets = await client.gadgets.list();
  assert.equal(Number(gadgets.meta.totalPages), 2);
  assert.deepEqual(gadgets.items.map((gadget) => gadget.id), ["g1", "g2"]);
  assert.deepEqual(gadgets.body.items.map((gadget) => gadget.id), ["g1", "g2"]);
  const lastGadgets = await gadgets.getNextPage();
  assert.deepEqual(lastGadgets.items.map((gadget) => gadget.id), ["g3"]);
  assert.ok(!lastGadgets.hasNextPage());

  const records = await client.records.list();
  assert.equal(Number(records.total), 3);
  assert.deepEqual(records.data.map((entry) => entry.id), ["r1", "r2"]);
  const pageSizes = [];
  for await (const page of records.iterPages()) {
    pageSizes.push(page.items.length);
  }
  assert.deepEqual(pageSizes, [2, 1]);

  const { data: health, response, requestId } = await client.account
    .checkHealth()
    .withResponse();
  assert.equal(health.status, "||");
  assert.equal(response.status, 200);
  assert.equal(requestId, "req_mock");
  assert.equal((await client.account.checkHealth().asResponse()).headers.get("x-request-id"), "req_mock");

  const request = { prompt: "hey" };
  assert.equal((await client.streaming.createCompletion(request)).text, "HEY");
  const stream = await client.streaming.createCompletionStream(request);
  const deltas = [];
  for await (const chunk of stream) {
    // The server numbers the characters, a property the spec does not declare.
    assert.equal((chunk as unknown as { index: number }).index, deltas.length);
    deltas.push(chunk.delta);
    assert.equal(stream.lastEvent?.event, "message");
  }
  assert.deepEqual(deltas, ["h", "e", "y"]);
  assert.deepEqual(request, { prompt: "hey" });

  const events = [];
  for await (const event of await client.streaming.retrieveEventsStream({ topic: "news" })) {
    events.push(event);
  }
  assert.deepEqual(events, [
    { event: "greeting", data: "news", id: "1", retry: undefined },
    { event: "message", data: "line1\nline2", id: "1", retry: undefined },
    { event: "message", data: '{"n": 3}', id: "3", retry: 1500 },
  ]);
  const uploaded = await client.streaming.uploadFile({
    file: { data: new TextEncoder().encode("hello"), filename: "a.txt", contentType: "text/plain" },
    name: "doc",
    count: 2,
    meta: { status: "ok" },
    tags: ["a", "b"],
  });
  assert.equal(
    uploaded.status,
    'count=::2;file=a.txt:text/plain:hello;meta=:application/json:{"status":"ok"};name=::doc;tags=::a;tags=::b'
  );
  const rawUpload = await client.streaming.uploadContent("f1", new Blob(["raw bytes"]));
  assert.equal(rawUpload.status, "application/octet-stream:raw bytes");
  const streamed = new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(new TextEncoder().encode("streamed"));
      controller.close();
    },
  });
  assert.equal(
    (await client.streaming.uploadContent("f1", streamed)).status,
    "application/octet-stream:streamed"
  );
  const searched = await client.wire.search({
    filter: { status: "open", amount: { gte: 5n } },
    expand: ["a", "b"],
    metadata: { k: "v" },
    ids: ["x", "y"],
    tags: ["t1", "t2"],
    range: { gte: 1n, lt: 9n },
    created: { gte: 3n, lt: 7n },
  });
  assert.equal(
    searched.status,
    "created[gte]=3&created[lt]=7&expand[]=a&expand[]=b&filter[amount][gte]=5&filter[status]=open&ids=x&ids=y" +
      "&metadata[k]=v&range[gte]=1&range[lt]=9&tags=t1,t2"
  );
  assert.equal((await client.wire.search({ created: 5n })).status, "created=5");
  const charged = await client.wire.createCharge({
    amount: 100n,
    capture: true,
    metadata: { order: "7" },
    items: [{ price: "p1", quantity: 2n }, { price: "p2" }],
    expand: ["customer"],
    statuses: ["a", "b"],
    codes: ["c1", "c2"],
    shipping: { address: { line1: "1 Main", city: "Paris" } },
  });
  assert.equal(
    charged.status,
    "application/x-www-form-urlencoded|amount=100&capture=true&codes=c1,c2&expand[]=customer" +
      "&items[0][price]=p1&items[0][quantity]=2&items[1][price]=p2&metadata[order]=7" +
      "&shipping[address][city]=Paris&shipping[address][line1]=1 Main&statuses=a&statuses=b"
  );
  assert.equal(
    (await client.wire.betaSearch({ limit: 2, features: ["x", "y"] })).status,
    "beta=true&limit=2|features=x,y"
  );
  const image = await client.wire.updateImage(42n, new TextEncoder().encode("png"));
  assert.equal(image.status, "42:image/png:png");

  await items(client);
  await content(client);
  await encoding(client);
  await cookies(client, baseURL);
  await oauth(baseURL);
  await retries(client);
  await errors(client, baseURL);
  await streaming(client);
  console.log("typescript smoke test passed");
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});

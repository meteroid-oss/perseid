import { strict as assert } from "node:assert";
import {
  APIConnectionError,
  AuthenticationError,
  Features,
  FeaturesError,
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

async function main() {
  const client = new Features({ apiKey: "tok", baseURL });
  assert.equal((await client.account.checkHealth()).status, "||");
  assert.equal((await client.account.retrieveMachine()).status, "Bearer tok||");
  assert.deepEqual(await collect(client.widgets.list()), ["w1", "w2", "w3"]);
  assert.deepEqual(
    await collect(client.widgets.listEvents("w1", { kind: "created" })),
    ["e1", "e2", "e3"]
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
    assert.deepEqual(error.error, { error: "unauthorized" });
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
  assert.equal((first.items[0] as Widget & { color?: string }).color, "red");
  assert.ok(first.hasNextPage());
  const second = await first.getNextPage();
  assert.deepEqual(second.items.map((widget: Widget) => widget.id), ["w3"]);
  assert.equal(second.body.nextCursor, null);
  assert.ok(!second.hasNextPage());
  const pageIds = [];
  for await (const page of first.iterPages()) {
    pageIds.push(page.items.length);
  }
  assert.deepEqual(pageIds, [2, 1]);
  assert.deepEqual(await collect(client.widgets.list()), ["w1", "w2", "w3"]);

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
  const content = await client.streaming.uploadContent("f1", new Blob(["raw bytes"]));
  assert.equal(content.status, "application/octet-stream:raw bytes");
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
    filter: { status: "open", amount: { gte: 5 } },
    expand: ["a", "b"],
    metadata: { k: "v" },
    ids: ["x", "y"],
    tags: ["t1", "t2"],
    range: { gte: 1, lt: 9 },
  });
  assert.equal(
    searched.status,
    "expand[]=a&expand[]=b&filter[amount][gte]=5&filter[status]=open&ids=x&ids=y" +
      "&metadata[k]=v&range[gte]=1&range[lt]=9&tags=t1,t2"
  );
  const charged = await client.wire.createCharge({
    amount: 100,
    capture: true,
    metadata: { order: "7" },
    items: [{ price: "p1", quantity: 2 }, { price: "p2" }],
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
  const image = await client.wire.updateImage(42, new TextEncoder().encode("png"));
  assert.equal(image.status, "42:image/png:png");
  console.log("typescript smoke test passed");
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});

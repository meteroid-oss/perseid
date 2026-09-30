import { strict as assert } from "node:assert";
import { Features } from "../src";

const serverUrl = process.env.FEATURES_URL!;

async function collect<T extends { id: string }>(items: AsyncIterable<T>): Promise<string[]> {
  const ids: string[] = [];
  for await (const item of items) {
    ids.push(item.id);
  }
  return ids;
}

async function main() {
  const client = new Features("tok", { serverUrl });
  assert.equal((await client.account.health()).status, "||");
  assert.equal((await client.account.machineStatus()).status, "Bearer tok||");
  assert.deepEqual(await collect(client.widgets.listWidgetsIter()), ["w1", "w2", "w3"]);
  assert.deepEqual(
    await collect(client.widgets.listWidgetEventsIter("w1", { kind: "created" })),
    ["e1", "e2", "e3"]
  );
  assert.deepEqual(await collect(client.gadgets.listGadgetsIter()), ["g1", "g2", "g3"]);
  assert.deepEqual(await collect(client.records.listRecordsIter()), ["r1", "r2", "r3"]);

  const basic = new Features(null, { serverUrl, basicAuth: { username: "u", password: "p" } });
  assert.equal((await basic.account.createSession()).status, "Basic dTpw||");

  const provided = new Features(null, { serverUrl, tokenProvider: async () => "fresh" });
  assert.equal((await provided.account.machineStatus()).status, "Bearer fresh||");

  const keyed = new Features(null, { serverUrl, apiKeys: { apiKey: "k" } });
  assert.deepEqual(await collect(keyed.widgets.listWidgetsIter()), ["w1", "w2", "w3"]);
  await assert.rejects(new Features(null, { serverUrl }).widgets.listWidgets());

  const events = [];
  for await (const event of await client.streaming.streamEvents({ topic: "news" })) {
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
  });
  assert.equal(
    uploaded.status,
    'count=::2;file=a.txt:text/plain:hello;meta=:application/json:{"status":"ok"};name=::doc'
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
  console.log("typescript smoke test passed");
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});

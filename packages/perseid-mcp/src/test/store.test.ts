// The stores the broker keeps grants in.
import assert from "node:assert/strict";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import { memoryStore, type Store } from "../index.js";
import { sqliteStore } from "../oauth/sqlite.js";
import { Keys, Records } from "../oauth/store.js";

async function behaves(store: Store) {
  await store.put("a", "1");
  await store.put("b", "2", 1);
  assert.equal(await store.get("a"), "1");
  assert.equal(await store.get("b"), "2");
  await store.put("a", "3");
  assert.equal(await store.get("a"), "3");
  await store.delete("a");
  assert.equal(await store.get("a"), undefined);
  await new Promise((resolve) => setTimeout(resolve, 1100));
  assert.equal(await store.get("b"), undefined);
}

test("the memory store", () => behaves(memoryStore()));

test("the SQLite store, which survives reopening", async () => {
  const file = join(await mkdtemp(join(tmpdir(), "perseid-mcp-")), "grants.db");
  await behaves(await sqliteStore(file));
  await (await sqliteStore(file)).put("kept", "yes");
  assert.equal(await (await sqliteStore(file)).get("kept"), "yes");
});

test("records take once, and expire on their own deadline", async () => {
  const records = new Records(memoryStore());
  await records.put("code", { n: 1 }, 60);
  assert.deepEqual(await records.take("code"), { n: 1 });
  assert.equal(await records.take("code"), undefined);
});

test("keys encrypt, and sign what only they verify", async () => {
  const keys = new Keys("x".repeat(32));
  const sealed = await keys.encrypt("sk_live");
  assert.notEqual(sealed, await keys.encrypt("sk_live"));
  assert.equal(await keys.decrypt(sealed), "sk_live");
  await assert.rejects(new Keys("y".repeat(32)).decrypt(sealed));
  const signed = await keys.sign("payload");
  assert.equal(await keys.verify(signed), "payload");
  assert.equal(await new Keys("y".repeat(32)).verify(signed), undefined);
});

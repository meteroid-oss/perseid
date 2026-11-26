// A store in a SQLite file, through node:sqlite (Node 22.5 and later): the broker's grants survive
// restarts. One server process per file.
import type { Store } from "./store.js";

export async function sqliteStore(path: string): Promise<Store> {
  const { DatabaseSync } = await import("node:sqlite");
  const db = new DatabaseSync(path);
  db.exec("PRAGMA journal_mode = WAL");
  db.exec("CREATE TABLE IF NOT EXISTS perseid_mcp (key TEXT PRIMARY KEY, value TEXT NOT NULL, expires INTEGER)");
  const get = db.prepare("SELECT value, expires FROM perseid_mcp WHERE key = ?");
  const put = db.prepare(
    "INSERT INTO perseid_mcp (key, value, expires) VALUES (?, ?, ?) ON CONFLICT (key) DO UPDATE SET value = excluded.value, expires = excluded.expires",
  );
  const remove = db.prepare("DELETE FROM perseid_mcp WHERE key = ?");
  const take = db.prepare("DELETE FROM perseid_mcp WHERE key = ? RETURNING value, expires");
  const sweep = db.prepare("DELETE FROM perseid_mcp WHERE expires IS NOT NULL AND expires <= ?");
  const timer = setInterval(() => sweep.run(Date.now()), 10 * 60 * 1000);
  timer.unref();
  return {
    async get(key) {
      const row = get.get(key) as { value: string; expires: number | null } | undefined;
      if (!row || (row.expires !== null && row.expires <= Date.now())) return undefined;
      return row.value;
    },
    async put(key, value, ttl) {
      put.run(key, value, ttl === undefined ? null : Date.now() + ttl * 1000);
    },
    async delete(key) {
      remove.run(key);
    },
    async take(key) {
      const row = take.get(key) as { value: string; expires: number | null } | undefined;
      if (!row || (row.expires !== null && row.expires <= Date.now())) return undefined;
      return row.value;
    },
  };
}

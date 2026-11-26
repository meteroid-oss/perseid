// Where the broker keeps clients, grants and tokens: any key-value store with expiry. Tokens are
// kept as their SHA-256 only, and API keys encrypted (AES-GCM) under a key derived from the
// owner's secret, so the store alone reveals neither.

export interface Store {
  get(key: string): Promise<string | undefined>;
  /** Writes `value`, gone after `ttl` seconds when given. */
  put(key: string, value: string, ttl?: number): Promise<void>;
  delete(key: string): Promise<void>;
  /** Reads and deletes at once, so that a code or a refresh token serves once even when two
   * requests race. Stores without it (KV) fall back to a read then a delete. */
  take?(key: string): Promise<string | undefined>;
}

/** A store in memory: one process, lost on restart. For tests, and stdio servers. */
export function memoryStore(): Store {
  const entries = new Map<string, { value: string; expires?: number }>();
  let writes = 0;
  const get = (key: string) => {
    const entry = entries.get(key);
    if (entry?.expires !== undefined && entry.expires <= Date.now()) {
      entries.delete(key);
      return undefined;
    }
    return entry?.value;
  };
  return {
    get: async (key) => get(key),
    async put(key, value, ttl) {
      // Every thousand writes, expired entries go.
      if (++writes % 1000 === 0) for (const k of [...entries.keys()]) get(k);
      entries.set(key, { value, expires: ttl === undefined ? undefined : Date.now() + ttl * 1000 });
    },
    async delete(key) {
      entries.delete(key);
    },
    async take(key) {
      const value = get(key);
      entries.delete(key);
      return value;
    },
  };
}

/** A Cloudflare D1 database as a store: one table, strongly consistent, so codes and refresh
 * tokens serve once across locations, which KV cannot promise. */
export function d1Store(db: {
  prepare(sql: string): { bind(...values: unknown[]): { first<T>(): Promise<T | null>; run(): Promise<unknown> } };
  exec(sql: string): Promise<unknown>;
}): Store {
  let ready: Promise<unknown> | undefined;
  const table = () =>
    (ready ??= db.exec("CREATE TABLE IF NOT EXISTS perseid_mcp (key TEXT PRIMARY KEY, value TEXT NOT NULL, expires INTEGER)"));
  const live = (row: { value: string; expires: number | null } | null) =>
    row && (row.expires === null || row.expires > Date.now()) ? row.value : undefined;
  return {
    async get(key) {
      await table();
      return live(await db.prepare("SELECT value, expires FROM perseid_mcp WHERE key = ?").bind(key).first());
    },
    async put(key, value, ttl) {
      await table();
      await db
        .prepare(
          "INSERT INTO perseid_mcp (key, value, expires) VALUES (?, ?, ?) ON CONFLICT (key) DO UPDATE SET value = excluded.value, expires = excluded.expires",
        )
        .bind(key, value, ttl === undefined ? null : Date.now() + ttl * 1000)
        .run();
      if (Math.random() < 0.01) await db.prepare("DELETE FROM perseid_mcp WHERE expires <= ?").bind(Date.now()).run();
    },
    async delete(key) {
      await table();
      await db.prepare("DELETE FROM perseid_mcp WHERE key = ?").bind(key).run();
    },
    async take(key) {
      await table();
      return live(await db.prepare("DELETE FROM perseid_mcp WHERE key = ? RETURNING value, expires").bind(key).first());
    },
  };
}

/** A Cloudflare KV namespace as a store: its `get`, `put` with `expirationTtl`, `delete`.
 * Eventually consistent: a code or refresh token raced from two locations may serve twice;
 * `d1Store` does not. */
export function kvStore(kv: {
  get(key: string): Promise<string | null>;
  put(key: string, value: string, options?: { expirationTtl?: number }): Promise<void>;
  delete(key: string): Promise<void>;
}): Store {
  return {
    get: async (key) => (await kv.get(key)) ?? undefined,
    // KV's shortest expiry is 60 seconds: shorter-lived entries also carry their own deadline.
    put: (key, value, ttl) => kv.put(key, value, ttl === undefined ? undefined : { expirationTtl: Math.max(60, ttl) }),
    delete: (key) => kv.delete(key),
  };
}

/** Reads and writes JSON records with a deadline of their own, whatever the store's precision. */
export class Records {
  constructor(private store: Store) {}

  async get<T>(key: string): Promise<T | undefined> {
    const text = await this.store.get(key);
    if (text === undefined) return undefined;
    const record = JSON.parse(text) as { value: T; expires?: number };
    if (record.expires !== undefined && record.expires <= Date.now()) {
      await this.store.delete(key);
      return undefined;
    }
    return record.value;
  }

  async put(key: string, value: unknown, ttl?: number): Promise<void> {
    const expires = ttl === undefined ? undefined : Date.now() + ttl * 1000;
    await this.store.put(key, JSON.stringify({ value, expires }), ttl);
  }

  /** Reads and deletes: a code or a refresh token serves once. */
  async take<T>(key: string): Promise<T | undefined> {
    if (this.store.take) {
      const text = await this.store.take(key);
      if (text === undefined) return undefined;
      const record = JSON.parse(text) as { value: T; expires?: number };
      return record.expires !== undefined && record.expires <= Date.now() ? undefined : record.value;
    }
    const value = await this.get<T>(key);
    if (value !== undefined) await this.store.delete(key);
    return value;
  }

  delete(key: string): Promise<void> {
    return this.store.delete(key);
  }
}

const encoder = new TextEncoder();

/** A random token, 32 bytes as base64url. */
export function randomToken(): string {
  return base64url(crypto.getRandomValues(new Uint8Array(32)));
}

export async function sha256(text: string): Promise<string> {
  return base64url(new Uint8Array(await crypto.subtle.digest("SHA-256", encoder.encode(text))));
}

export function base64url(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function unbase64url(text: string): Uint8Array {
  const binary = atob(text.replace(/-/g, "+").replace(/_/g, "/"));
  return Uint8Array.from(binary, (c) => c.charCodeAt(0));
}

/** Encryption and signatures under keys derived (HKDF) from the owner's secret. */
export class Keys {
  private aes: Promise<CryptoKey>;
  private hmac: Promise<CryptoKey>;

  constructor(secret: string) {
    if (secret.length < 32) throw new Error("the OAuth secret must be at least 32 characters: generate one with `openssl rand -base64 32`");
    const base = crypto.subtle.importKey("raw", encoder.encode(secret), "HKDF", false, ["deriveKey"]);
    const derive = (info: string, algorithm: AesKeyGenParams | HmacKeyGenParams, usages: KeyUsage[]) =>
      base.then((key) =>
        crypto.subtle.deriveKey(
          { name: "HKDF", hash: "SHA-256", salt: new Uint8Array(32), info: encoder.encode(info) },
          key,
          algorithm,
          false,
          usages,
        ),
      );
    this.aes = derive("perseid-mcp api keys", { name: "AES-GCM", length: 256 }, ["encrypt", "decrypt"]);
    this.hmac = derive("perseid-mcp requests", { name: "HMAC", hash: "SHA-256", length: 256 }, ["sign", "verify"]);
  }

  async encrypt(text: string): Promise<string> {
    const iv = crypto.getRandomValues(new Uint8Array(12));
    const sealed = await crypto.subtle.encrypt({ name: "AES-GCM", iv }, await this.aes, encoder.encode(text));
    return `${base64url(iv)}.${base64url(new Uint8Array(sealed))}`;
  }

  async decrypt(sealed: string): Promise<string> {
    const [iv, data] = sealed.split(".");
    const plain = await crypto.subtle.decrypt(
      { name: "AES-GCM", iv: unbase64url(iv ?? "") as BufferSource },
      await this.aes,
      unbase64url(data ?? "") as BufferSource,
    );
    return new TextDecoder().decode(plain);
  }

  /** `payload` with its signature, for a form to carry back. */
  async sign(payload: string): Promise<string> {
    const signature = await crypto.subtle.sign("HMAC", await this.hmac, encoder.encode(payload));
    return `${base64url(encoder.encode(payload))}.${base64url(new Uint8Array(signature))}`;
  }

  /** The payload of what `sign` gave, if its signature holds. */
  async verify(signed: string): Promise<string | undefined> {
    const [payload, signature] = signed.split(".");
    if (!payload || !signature) return undefined;
    const bytes = unbase64url(payload);
    const valid = await crypto.subtle.verify("HMAC", await this.hmac, unbase64url(signature) as BufferSource, bytes as BufferSource);
    return valid ? new TextDecoder().decode(bytes) : undefined;
  }
}

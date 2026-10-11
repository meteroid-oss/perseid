// The OAuth broker and the resource server, through the HTTP handler: registration, consent,
// tokens, refresh, revocation, and MCP calls made with the key a token maps to.
import assert from "node:assert/strict";
import { describe, test } from "node:test";

import { createHandler, memoryStore, oauthBroker, resourceServer, type Api, type BrokerOptions } from "../index.js";
import { base64url, sha256 } from "../oauth/store.js";
import { quickjsSandbox } from "../sandbox/quickjs.js";

const ORIGIN = "https://mcp.acme.test";
const REDIRECT = "https://client.test/callback";
const SECRET = "a-secret-of-at-least-thirty-two-characters";

/** An API whose one operation returns the key the client was made with. */
const api: Api = {
  name: "Acme",
  server: "acme-mcp",
  version: "1.0.0",
  envPrefix: "ACME",
  instructions: "",
  corpus: {
    package: "acme",
    operations: [
      {
        accessor: "whoami.retrieve",
        signature: "whoami.retrieve(): Promise<string>",
        method: "GET",
        path: "/whoami",
        summary: "",
        description: "",
        deprecated: false,
        paginated: false,
        types: [],
      },
    ],
    models: {},
  },
  client: (options) => ({ whoami: { retrieve: async () => options.apiKey } }),
};

function server(options: Partial<BrokerOptions> = {}) {
  const handler = createHandler(api, {
    sandbox: quickjsSandbox(),
    auth: oauthBroker(api, { store: memoryStore(), secret: SECRET, ...options }),
  });
  return (path: string, init: RequestInit = {}) => handler(new Request(`${ORIGIN}${path}`, { redirect: "manual", ...init }));
}

type Send = ReturnType<typeof server>;

const form = (fields: Record<string, string>) => ({
  method: "POST",
  headers: { "content-type": "application/x-www-form-urlencoded" },
  body: new URLSearchParams(fields),
});

/** The MCP call returning the key the server called the API with. */
async function whoami(send: Send, token: string | undefined): Promise<Response> {
  return send("/mcp", {
    method: "POST",
    headers: {
      "content-type": "application/json",
      accept: "application/json, text/event-stream",
      ...(token ? { authorization: `Bearer ${token}` } : {}),
    },
    body: JSON.stringify({
      jsonrpc: "2.0",
      id: 1,
      method: "tools/call",
      params: { name: "execute", arguments: { code: "return await client.whoami.retrieve();" } },
    }),
  });
}

async function keyOf(response: Response): Promise<unknown> {
  const body = (await response.json()) as { result: { content: { text: string }[] } };
  return JSON.parse(body.result.content[0]!.text).result;
}

async function pkce() {
  const verifier = base64url(crypto.getRandomValues(new Uint8Array(32)));
  return { verifier, challenge: await sha256(verifier) };
}

async function register(send: Send, redirect = REDIRECT): Promise<string> {
  const response = await send("/register", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ redirect_uris: [redirect], client_name: "Test client" }),
  });
  assert.equal(response.status, 201);
  return ((await response.json()) as { client_id: string }).client_id;
}

function authorizeUrl(clientId: string, challenge: string, extra: Record<string, string> = {}) {
  const params = new URLSearchParams({
    response_type: "code",
    client_id: clientId,
    redirect_uri: REDIRECT,
    code_challenge: challenge,
    code_challenge_method: "S256",
    state: "xyz",
    resource: `${ORIGIN}/mcp`,
    ...extra,
  });
  return `/authorize?${params}`;
}

/** The consent page's signed request. */
async function pending(response: Response): Promise<string> {
  assert.equal(response.status, 200);
  const html = await response.text();
  return /name="pending" value="([^"]+)"/.exec(html)![1]!;
}

/** Registers, consents with `key` and exchanges the code: the client's tokens. */
async function connect(send: Send, key = "sk_live_123") {
  const clientId = await register(send);
  const { verifier, challenge } = await pkce();
  const signed = await pending(await send(authorizeUrl(clientId, challenge)));
  const consented = await send("/authorize", form({ pending: signed, api_key: key, action: "allow" }));
  assert.equal(consented.status, 302);
  const location = new URL(consented.headers.get("location")!);
  assert.equal(`${location.origin}${location.pathname}`, REDIRECT);
  assert.equal(location.searchParams.get("state"), "xyz");
  assert.equal(location.searchParams.get("iss"), ORIGIN);
  const code = location.searchParams.get("code")!;
  const tokens = await send(
    "/token",
    form({ grant_type: "authorization_code", code, redirect_uri: REDIRECT, client_id: clientId, code_verifier: verifier }),
  );
  assert.equal(tokens.status, 200);
  const body = (await tokens.json()) as { access_token: string; refresh_token: string; token_type: string };
  assert.equal(body.token_type, "Bearer");
  return { ...body, clientId, code, verifier };
}

describe("the OAuth broker", () => {
  test("publishes its metadata, and points unauthenticated requests at it", async () => {
    const send = server();
    const resource = (await (await send("/.well-known/oauth-protected-resource")).json()) as Record<string, unknown>;
    assert.deepEqual(resource.resource, `${ORIGIN}/mcp`);
    assert.deepEqual(resource.authorization_servers, [ORIGIN]);
    const metadata = (await (await send("/.well-known/oauth-authorization-server")).json()) as Record<string, unknown>;
    assert.equal(metadata.issuer, ORIGIN);
    assert.deepEqual(metadata.code_challenge_methods_supported, ["S256"]);
    assert.equal(metadata.client_id_metadata_document_supported, true);
    const anonymous = await whoami(send, undefined);
    assert.equal(anonymous.status, 401);
    assert.match(anonymous.headers.get("www-authenticate")!, /resource_metadata="https:\/\/mcp\.acme\.test\/\.well-known\/oauth-protected-resource"/);
  });

  test("calls the API with the key the user consented with", async () => {
    const send = server();
    const { access_token } = await connect(send, "sk_live_123");
    const response = await whoami(send, access_token);
    assert.equal(response.status, 200);
    assert.equal(await keyOf(response), "sk_live_123");
    assert.equal((await whoami(send, "made-up")).status, 401);
  });

  test("serves a code once, to its client, with its verifier", async () => {
    const send = server();
    const { code, clientId, verifier } = await connect(send);
    const again = await send(
      "/token",
      form({ grant_type: "authorization_code", code, redirect_uri: REDIRECT, client_id: clientId, code_verifier: verifier }),
    );
    assert.equal(again.status, 400);
    assert.equal(((await again.json()) as { error: string }).error, "invalid_grant");

    const other = await register(send);
    const { challenge } = await pkce();
    const signed = await pending(await send(authorizeUrl(other, challenge)));
    const location = new URL((await send("/authorize", form({ pending: signed, api_key: "k", action: "allow" }))).headers.get("location")!);
    const wrong = await send(
      "/token",
      form({ grant_type: "authorization_code", code: location.searchParams.get("code")!, redirect_uri: REDIRECT, client_id: other, code_verifier: "x".repeat(43) }),
    );
    assert.equal(wrong.status, 400);
    assert.match(((await wrong.json()) as { error_description: string }).error_description, /code_verifier/);
  });

  test("rotates refresh tokens, and revokes a grant whole", async () => {
    const send = server();
    const first = await connect(send);
    const refreshed = await send("/token", form({ grant_type: "refresh_token", refresh_token: first.refresh_token, client_id: first.clientId }));
    assert.equal(refreshed.status, 200);
    const second = (await refreshed.json()) as { access_token: string; refresh_token: string };
    assert.notEqual(second.refresh_token, first.refresh_token);
    assert.equal(await keyOf(await whoami(send, second.access_token)), "sk_live_123");

    assert.equal((await send("/revoke", form({ token: second.access_token }))).status, 200);
    assert.equal((await whoami(send, second.access_token)).status, 401);
    assert.equal((await whoami(send, first.access_token)).status, 401);
    const after = await send("/token", form({ grant_type: "refresh_token", refresh_token: second.refresh_token, client_id: first.clientId }));
    assert.equal(after.status, 400);
  });

  test("refuses redirects a client did not register, requests without PKCE, and forged forms", async () => {
    const send = server();
    const clientId = await register(send);
    const { challenge } = await pkce();
    const elsewhere = await send(authorizeUrl(clientId, challenge, { redirect_uri: "https://evil.test/cb" }));
    assert.equal(elsewhere.status, 400);
    assert.equal(elsewhere.headers.get("location"), null);
    const plain = await send(authorizeUrl(clientId, challenge, { code_challenge_method: "plain" }));
    assert.equal(plain.status, 302);
    assert.equal(new URL(plain.headers.get("location")!).searchParams.get("error"), "invalid_request");
    const otherResource = await send(authorizeUrl(clientId, challenge, { resource: "https://other.test/mcp" }));
    assert.equal(new URL(otherResource.headers.get("location")!).searchParams.get("error"), "invalid_target");
    const signed = await pending(await send(authorizeUrl(clientId, challenge)));
    const forged = await send("/authorize", form({ pending: signed.replace(/^./, "x"), api_key: "k", action: "allow" }));
    assert.equal(forged.status, 400);
    const denied = await send("/authorize", form({ pending: signed, action: "deny" }));
    assert.equal(new URL(denied.headers.get("location")!).searchParams.get("error"), "access_denied");
    const http = await send("/register", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ redirect_uris: ["http://example.com/cb"] }),
    });
    assert.equal(http.status, 400);
  });

  test("shows a consent page that is not framed, nor runs scripts, and says whose claim the name is", async () => {
    const send = server({ keysUrl: "https://acme.test/keys" });
    const clientId = await register(send);
    const { challenge } = await pkce();
    const response = await send(authorizeUrl(clientId, challenge));
    assert.match(response.headers.get("content-security-policy")!, /default-src 'none'.*frame-ancestors 'none'/);
    const html = await response.text();
    assert.match(html, /Connect <strong>Test client<\/strong> to Acme/);
    assert.match(html, /the application's own claim/);
    assert.match(html, /href="https:\/\/acme\.test\/keys"/);
    assert.doesNotMatch(html, /<script/);
  });

  test("checks a key before granting, when told how", async () => {
    const send = server({ validate: async (key) => (key.startsWith("sk_") ? undefined : "This is not an Acme key.") });
    const clientId = await register(send);
    const { challenge } = await pkce();
    const signed = await pending(await send(authorizeUrl(clientId, challenge)));
    const refused = await send("/authorize", form({ pending: signed, api_key: "nope", action: "allow" }));
    assert.equal(refused.status, 400);
    assert.match(await refused.text(), /This is not an Acme key\./);
    const accepted = await send("/authorize", form({ pending: signed, api_key: "sk_ok", action: "allow" }));
    assert.equal(accepted.status, 302);
  });

  test("accepts clients identified by a metadata document at their client_id", async () => {
    const id = "https://app.example.com/oauth/client.json";
    const documents: Record<string, unknown> = {
      [id]: { client_id: id, client_name: "Example app", redirect_uris: [REDIRECT] },
      "https://liar.example.com/client.json": { client_id: "https://other.example.com/client.json", redirect_uris: [REDIRECT] },
    };
    const fetched: string[] = [];
    const send = server({
      fetch: (async (input: string | URL | Request) => {
        const url = String(input);
        fetched.push(url);
        return documents[url] ? new Response(JSON.stringify(documents[url])) : new Response("", { status: 404 });
      }) as typeof fetch,
    });
    const { challenge } = await pkce();
    const html = await (await send(authorizeUrl(id, challenge))).text();
    assert.match(html, /Identified by <strong>app\.example\.com<\/strong>/);
    assert.equal((await send(authorizeUrl("https://liar.example.com/client.json", challenge))).status, 400);
    assert.equal((await send(authorizeUrl("https://127.0.0.1/client.json", challenge))).status, 400);
    assert.equal((await send(authorizeUrl("https://localhost/client.json", challenge))).status, 400);
    assert.ok(!fetched.some((url) => /127\.0\.0\.1|localhost/.test(url)), "never fetches a private address");
  });

  test("keeps tokens hashed and keys encrypted", async () => {
    const store = memoryStore();
    const writes: string[] = [];
    const spy = { ...store, put: (key: string, value: string, ttl?: number) => (writes.push(key + " " + value), store.put(key, value, ttl)) };
    const send = server({ store: spy });
    const { access_token, refresh_token } = await connect(send, "sk_live_secret");
    const all = writes.join("\n");
    assert.ok(!all.includes("sk_live_secret"));
    assert.ok(!all.includes(access_token));
    assert.ok(!all.includes(refresh_token));
  });

  test("needs a secret of 32 characters or more", () => {
    assert.throws(() => oauthBroker(api, { store: memoryStore(), secret: "short" }), /at least 32 characters/);
  });
});

describe("the OAuth broker, against the attacks a review found", () => {
  test("names the host the code actually goes to, and keeps a client's redirects on one host", async () => {
    const send = server();
    const mixed = await send("/register", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ client_name: "Claude", redirect_uris: ["https://claude.ai/cb", "https://evil.example/cb"] }),
    });
    assert.equal(mixed.status, 400);
    const clientId = await register(send);
    const { challenge } = await pkce();
    const html = await (await send(authorizeUrl(clientId, challenge))).text();
    assert.match(html, /It returns to <strong>client\.test<\/strong>/);
  });

  test("ends a grant whose refresh token is used twice", async () => {
    const send = server();
    const first = await connect(send);
    const refresh = (token: string) => send("/token", form({ grant_type: "refresh_token", refresh_token: token, client_id: first.clientId }));
    const second = (await (await refresh(first.refresh_token)).json()) as { access_token: string; refresh_token: string };
    assert.equal((await refresh(first.refresh_token)).status, 400, "the stolen token, replayed");
    assert.equal((await whoami(send, second.access_token)).status, 401, "the whole grant is revoked");
    assert.equal((await refresh(second.refresh_token)).status, 400);
  });

  test("ends a grant at its lifetime, refreshes or not", async () => {
    const send = server({ grantTtl: 2 });
    const first = await connect(send);
    await new Promise((resolve) => setTimeout(resolve, 2100));
    const refreshed = await send("/token", form({ grant_type: "refresh_token", refresh_token: first.refresh_token, client_id: first.clientId }));
    assert.equal(refreshed.status, 400);
  });

  test("redirects loopback codes over http only, to the registered path", async () => {
    const send = server();
    const loopback = "http://127.0.0.1:8080/cb";
    const clientId = await register(send, loopback);
    const { challenge } = await pkce();
    const port = await send(authorizeUrl(clientId, challenge, { redirect_uri: "http://127.0.0.1:9999/cb" }));
    assert.equal(port.status, 200, "any port on loopback");
    for (const uri of ["foo://127.0.0.1/cb", "https://127.0.0.1:8080/cb", "http://user@127.0.0.1:8080/cb", "http://127.0.0.1:8080/other"]) {
      assert.equal((await send(authorizeUrl(clientId, challenge, { redirect_uri: uri }))).status, 400, uri);
    }
  });

  test("refuses large bodies, and brakes registrations from one address", async () => {
    const send = server({ registrationsPerHour: 2 });
    const big = await send("/register", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ redirect_uris: [REDIRECT], client_name: "x".repeat(100_000) }),
    });
    assert.equal(big.status, 413);
    await register(send);
    await register(send);
    const third = await send("/register", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ redirect_uris: [REDIRECT] }),
    });
    assert.equal(third.status, 429);
  });

  test("fetches no metadata document from a private address, however it is spelled", async () => {
    const fetched: string[] = [];
    const send = server({
      fetch: (async (input: string | URL | Request) => {
        fetched.push(String(input));
        return new Response("{}");
      }) as typeof fetch,
    });
    const { challenge } = await pkce();
    for (const host of ["localhost.", "[::]", "metadata.google.internal.", "[64:ff9b::7f00:1]", "[::7f00:1]", "[fec0::1]", "127.0.0.1.nip.io", "[::ffff:7f00:1]", "2130706433", "198.18.0.1"]) {
      await send(authorizeUrl(`https://${host}/client.json`, challenge));
    }
    assert.deepEqual(fetched, []);
  });
});

describe("the resource server", () => {
  const introspected: string[] = [];
  const handler = createHandler(api, {
    sandbox: quickjsSandbox(),
    auth: resourceServer({
      authorizationServers: ["https://auth.acme.test"],
      introspection: { url: "https://auth.acme.test/introspect", clientId: "mcp", clientSecret: "s3cret" },
      fetch: (async (_url: string | URL | Request, init?: RequestInit) => {
        const token = new URLSearchParams(String(init?.body)).get("token")!;
        introspected.push(token);
        const answers: Record<string, unknown> = {
          good: { active: true, aud: `${ORIGIN}/mcp` },
          elsewhere: { active: true, aud: "https://other.test/mcp" },
          revoked: { active: false },
        };
        return new Response(JSON.stringify(answers[token] ?? { active: false }));
      }) as typeof fetch,
    }),
  });
  const send = (path: string, init: RequestInit = {}) => handler(new Request(`${ORIGIN}${path}`, init));

  test("points clients at the API's authorization server", async () => {
    const metadata = (await (await send("/.well-known/oauth-protected-resource")).json()) as Record<string, unknown>;
    assert.deepEqual(metadata.authorization_servers, ["https://auth.acme.test"]);
    assert.equal(metadata.resource, `${ORIGIN}/mcp`);
  });

  test("calls the API with active tokens issued for it, once introspected", async () => {
    assert.equal(await keyOf(await whoami(send, "good")), "good");
    assert.equal(await keyOf(await whoami(send, "good")), "good");
    assert.equal(introspected.filter((t) => t === "good").length, 1);
    assert.equal((await whoami(send, "revoked")).status, 401);
    assert.equal((await whoami(send, "elsewhere")).status, 401);
  });
});

// The OAuth2 client credentials flow of the SDK generated from tests/fixtures/oauth.yaml, against a
// fake fetch that plays the token endpoint and the API.
import assert from "node:assert/strict";
import { after, describe, it } from "node:test";
import { generate } from "./_sdk.mjs";

const { sdk, cleanup } = await generate("oauth.yaml", { name: "Vault" });
after(cleanup);

const json = (body, status = 200, headers = {}) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", ...headers },
  });
const TOKEN_URL = "https://vault.test/v1/oauth/token";
const basic = (id, secret) => `Basic ${Buffer.from(`${id}:${secret}`).toString("base64")}`;

/**
 * A fake server: `tokens` numbers the access tokens it issues (`at-1`, `at-2`...), and `revoked`
 * lists those the API rejects with a 401. `calls` records every request, the token endpoint's too.
 */
function server({ expiresIn = 3600, revoked = [], tokenResponses = [] } = {}) {
  const calls = [];
  let issued = 0;
  const fetch = async (url, init) => {
    const call = { url: String(url), method: init.method, headers: { ...init.headers }, body: init.body };
    calls.push(call);
    if (call.url === TOKEN_URL) {
      const scripted = tokenResponses.shift();
      if (scripted !== undefined) {
        return scripted;
      }
      issued++;
      return json({ access_token: `at-${issued}`, token_type: "Bearer", expires_in: expiresIn });
    }
    const token = call.headers.authorization?.replace("Bearer ", "");
    return revoked.includes(token) ? json({ message: "revoked" }, 401) : json({ status: token ?? "anonymous" });
  };
  return { calls, fetch, tokenCalls: () => calls.filter((call) => call.url === TOKEN_URL) };
}

const vault = (fetch, options = {}) =>
  new sdk.Vault({ fetch, clientId: "id", clientSecret: "secret", maxRetries: 0, ...options });

describe("OAuth2 client credentials", () => {
  it("fetches a token on first use and keeps it", async () => {
    const { calls, fetch, tokenCalls } = server();
    const client = vault(fetch);
    assert.equal((await client.account.retrieveMachine()).status, "at-1");
    assert.equal((await client.account.retrieveMachine()).status, "at-1");
    assert.equal(tokenCalls().length, 1);
    const [token] = tokenCalls();
    assert.equal(token.method, "POST");
    assert.equal(token.headers["content-type"], "application/x-www-form-urlencoded");
    assert.equal(token.headers.authorization, basic("id", "secret"));
    assert.deepEqual([...new URLSearchParams(token.body)], [
      ["grant_type", "client_credentials"],
      ["scope", "secrets.read secrets.write"],
    ]);
    assert.equal(calls[1].headers.authorization, "Bearer at-1");
  });

  it("asks for the scopes of every requirement, and sends nothing to public operations", async () => {
    const { fetch, tokenCalls } = server();
    const client = vault(fetch);
    assert.equal((await client.account.checkHealth()).status, "anonymous");
    assert.equal(tokenCalls().length, 0);
    assert.equal((await client.account.createSession()).status, "at-1");
    assert.equal(new URLSearchParams(tokenCalls()[0].body).get("scope"), "secrets.read secrets.write");
    assert.equal((await client.account.retrieveMachine()).status, "at-1", "one token serves every operation");
  });

  it("form-encodes the credentials before the basic header, or sends them in the body", async () => {
    const odd = server();
    await vault(odd.fetch, { clientId: "a b", clientSecret: "p@ss:word" }).account.retrieveMachine();
    assert.equal(odd.tokenCalls()[0].headers.authorization, basic("a+b", "p%40ss%3Aword"));

    const body = server();
    await vault(body.fetch, { oauthClientAuth: "body" }).account.retrieveMachine();
    const [token] = body.tokenCalls();
    assert.equal(token.headers.authorization, undefined);
    const form = new URLSearchParams(token.body);
    assert.equal(form.get("client_id"), "id");
    assert.equal(form.get("client_secret"), "secret");
  });

  it("renews a token that expired", async () => {
    const { fetch, tokenCalls } = server({ expiresIn: 0 });
    const client = vault(fetch);
    assert.equal((await client.account.retrieveMachine()).status, "at-1");
    assert.equal((await client.account.retrieveMachine()).status, "at-2");
    assert.equal(tokenCalls().length, 2);
  });

  it("replaces a token the API rejects, once", async () => {
    const rejected = server({ revoked: ["at-1"] });
    const client = vault(rejected.fetch);
    assert.equal((await client.account.retrieveMachine()).status, "at-2");
    assert.equal(rejected.tokenCalls().length, 2);
    assert.equal(rejected.calls.filter((call) => call.url !== TOKEN_URL).length, 2);
    assert.equal((await client.account.retrieveMachine()).status, "at-2");
    assert.equal(rejected.tokenCalls().length, 2);

    const always = server({ revoked: ["at-1", "at-2", "at-3"] });
    await assert.rejects(vault(always.fetch).account.retrieveMachine(), sdk.AuthenticationError);
    assert.equal(always.tokenCalls().length, 2, "a second 401 is the caller's");
  });

  it("lets the API key win over the client credentials", async () => {
    const { calls, fetch } = server();
    const client = new sdk.Vault({ fetch, apiKey: "static", clientId: "id", clientSecret: "secret", maxRetries: 0 });
    assert.equal((await client.account.createSession()).status, "static");
    assert.equal(calls.length, 1, "a token wins over the client credentials");
  });

  it("shares one token request among concurrent calls", async () => {
    const { fetch, tokenCalls } = server();
    const client = vault(fetch);
    const results = await Promise.all([1, 2, 3].map(() => client.account.retrieveMachine()));
    assert.deepEqual(results.map((result) => result.status), ["at-1", "at-1", "at-1"]);
    assert.equal(tokenCalls().length, 1);
  });

  it("retries the token request like any request", async () => {
    const { fetch, tokenCalls } = server({ tokenResponses: [json({ error: "busy" }, 503)] });
    const client = vault(fetch, { maxRetries: 1, retryScheduleInMs: [0] });
    assert.equal((await client.account.retrieveMachine()).status, "at-1");
    assert.equal(tokenCalls().length, 2);

    const failing = server({ tokenResponses: [json({ error: "invalid_client" }, 401)] });
    await assert.rejects(vault(failing.fetch).account.retrieveMachine(), (error) => {
      assert.ok(error instanceof sdk.AuthenticationError);
      assert.equal(error.error.error, "invalid_client");
      return true;
    });
    assert.equal(failing.calls.length, 1, "the API is not called without a token");

    const empty = server({ tokenResponses: [json({ token_type: "Bearer" })] });
    await assert.rejects(vault(empty.fetch).account.retrieveMachine(), sdk.VaultError);
  });

  it("resolves a relative token URL against the base URL", async () => {
    const { calls, fetch } = server();
    // The fake only plays the token endpoint of the default base URL.
    const other = vault(async (url, init) => fetch(String(url).replace("https://other.test/api", "https://vault.test/v1"), init), {
      baseURL: "https://other.test/api/",
    });
    await other.account.retrieveMachine();
    assert.equal(calls[0].url, TOKEN_URL);
  });

  it("lets a token provider or the API key win, and reads its credentials from the environment", async () => {
    const provided = server();
    const client = vault(provided.fetch, { tokenProvider: async () => "mine" });
    assert.equal((await client.account.retrieveMachine()).status, "mine");
    assert.equal(provided.tokenCalls().length, 0);

    process.env.VAULT_CLIENT_ID = "env-id";
    process.env.VAULT_CLIENT_SECRET = "env-secret";
    try {
      const env = server();
      const fromEnv = new sdk.Vault({ fetch: env.fetch, maxRetries: 0 });
      assert.equal((await fromEnv.account.retrieveMachine()).status, "at-1");
      assert.equal(env.tokenCalls()[0].headers.authorization, basic("env-id", "env-secret"));
    } finally {
      delete process.env.VAULT_CLIENT_ID;
      delete process.env.VAULT_CLIENT_SECRET;
    }
  });
});

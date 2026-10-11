// The OAuth broker, for APIs with API keys only: the MCP server is its own authorization server
// (MCP's authorization spec, OAuth 2.1). A client (claude.ai, ChatGPT) registers, by dynamic
// registration or a Client ID Metadata Document; the user pastes their API key once on a consent
// page; the client gets tokens; each MCP request's token maps to that key, kept encrypted.
import type { Api } from "../server.js";
import type { Auth } from "../http.js";
import { json } from "../http.js";
import { Keys, randomToken, Records, sha256, type Store } from "./store.js";
import { consentPage, errorPage } from "./pages.js";

export interface BrokerOptions {
  store: Store;
  /** At least 32 characters: the keys encrypting API keys and signing consent forms derive
   * from it. Changing it revokes every grant. */
  secret: string;
  /** The server's public URL, `https://mcp.acme.com`: the issuer and the resource. By default
   * the origin of each request, which a proxy must then forward faithfully. */
  url?: string;
  /** Checks a key on the consent page before granting: its error message, or nothing. */
  validate?: (apiKey: string) => Promise<string | undefined>;
  /** Where users find their API key, linked from the consent page. */
  keysUrl?: string;
  /** Lifetimes, in seconds: access tokens (1 hour), refresh tokens (30 days), and a grant
   * whatever its refreshes (90 days), after which the user consents again. */
  accessTtl?: number;
  refreshTtl?: number;
  grantTtl?: number;
  /** Dynamic registrations an address may make in an hour (20). */
  registrationsPerHour?: number;
  /** Fetches Client ID Metadata Documents: `fetch` by default. */
  fetch?: typeof fetch;
}

/** A registered client: its redirect URIs, and what the consent page shows of it. */
interface Client {
  client_id: string;
  client_name?: string;
  redirect_uris: string[];
  /** Registered dynamically: its name is the client's own claim. */
  dynamic: boolean;
}

/** What the user granted: the client, the API key it acts with, and until when. */
interface Grant {
  clientId: string;
  key: string;
  expires: number;
}

interface Code {
  grantId: string;
  clientId: string;
  redirectUri: string;
  challenge: string;
  resource: string;
}

interface Token {
  grantId: string;
  clientId: string;
}

/** The authorization request the consent form carries back, signed. */
interface Pending {
  clientId: string;
  redirectUri: string;
  state?: string;
  challenge: string;
  resource: string;
  expires: number;
}

const CODE_TTL = 60;
const CONSENT_TTL = 15 * 60;

export function oauthBroker(api: Api, options: BrokerOptions): Auth {
  const records = new Records(options.store);
  const keys = new Keys(options.secret);
  const accessTtl = options.accessTtl ?? 3600;
  const refreshTtl = options.refreshTtl ?? 30 * 24 * 3600;
  const grantTtl = options.grantTtl ?? 90 * 24 * 3600;
  const registrations = rateLimit(options.registrationsPerHour ?? 20, 3600 * 1000);
  const fetchDocument = options.fetch ?? fetch;
  const origin = (request: Request) => (options.url ?? new URL(request.url).origin).replace(/\/+$/, "");
  const resourceOf = (request: Request) => `${origin(request)}/mcp`;

  const metadata = (request: Request) => ({
    issuer: origin(request),
    authorization_endpoint: `${origin(request)}/authorize`,
    token_endpoint: `${origin(request)}/token`,
    registration_endpoint: `${origin(request)}/register`,
    revocation_endpoint: `${origin(request)}/revoke`,
    response_types_supported: ["code"],
    grant_types_supported: ["authorization_code", "refresh_token"],
    code_challenge_methods_supported: ["S256"],
    token_endpoint_auth_methods_supported: ["none"],
    // Client ID Metadata Documents first; dynamic registration, deprecated since MCP 2026-07-28,
    // as the fallback.
    client_id_metadata_document_supported: true,
    authorization_response_iss_parameter_supported: true,
    scopes_supported: [],
  });

  const resourceMetadata = (request: Request) => ({
    resource: resourceOf(request),
    authorization_servers: [origin(request)],
    bearer_methods_supported: ["header"],
    resource_name: `${api.name} MCP`,
  });

  /** A client by its id: registered here, or an https URL serving its metadata document. */
  async function client(clientId: string): Promise<Client | undefined> {
    const registered = await records.get<Client>(`client:${clientId}`);
    if (registered) return registered;
    if (!/^https:\/\//.test(clientId)) return undefined;
    const cached = await records.get<Client>(`cimd:${clientId}`);
    if (cached) return cached;
    if (await records.get<boolean>(`cimd-missing:${clientId}`)) return undefined;
    const document = await clientMetadataDocument(clientId, fetchDocument);
    if (document) await records.put(`cimd:${clientId}`, document, 3600);
    else await records.put(`cimd-missing:${clientId}`, true, 300);
    return document;
  }

  async function register(request: Request): Promise<Response> {
    if (!registrations(clientAddress(request))) {
      return oauthError(429, "too_many_requests", "too many registrations from this address: try again later");
    }
    let body: { redirect_uris?: unknown; client_name?: unknown; token_endpoint_auth_method?: unknown };
    try {
      body = await request.json();
    } catch {
      return oauthError(400, "invalid_client_metadata", "the body must be JSON");
    }
    const uris = Array.isArray(body.redirect_uris) ? body.redirect_uris.filter((u): u is string => typeof u === "string") : [];
    if (!uris.length || uris.length > 10 || !uris.every(redirectAllowed)) {
      return oauthError(400, "invalid_redirect_uri", "redirect_uris: https URLs, or http on a loopback address");
    }
    // The consent page names one host the code goes to: a client registers redirects on one.
    if (new Set(uris.map((uri) => new URL(uri).hostname)).size > 1) {
      return oauthError(400, "invalid_redirect_uri", "redirect_uris must all be on one host");
    }
    if (body.token_endpoint_auth_method !== undefined && body.token_endpoint_auth_method !== "none") {
      return oauthError(400, "invalid_client_metadata", "only public clients: token_endpoint_auth_method none, with PKCE");
    }
    const registered: Client = {
      client_id: randomToken(),
      client_name: typeof body.client_name === "string" ? body.client_name.slice(0, 100) : undefined,
      redirect_uris: uris,
      dynamic: true,
    };
    await records.put(`client:${registered.client_id}`, registered, 90 * 24 * 3600);
    return json(201, {
      client_id: registered.client_id,
      client_id_issued_at: Math.floor(Date.now() / 1000),
      client_name: registered.client_name,
      redirect_uris: registered.redirect_uris,
      grant_types: ["authorization_code", "refresh_token"],
      response_types: ["code"],
      token_endpoint_auth_method: "none",
    });
  }

  /** Checks an authorization request: the pending grant, or why it fails. A failure the client
   * can be told of carries its redirect. */
  async function check(
    request: Request,
    params: URLSearchParams,
  ): Promise<{ pending: Pending; client: Client } | { error: string; redirect?: string }> {
    const clientId = params.get("client_id") ?? "";
    const found = clientId ? await client(clientId) : undefined;
    if (!found) return { error: "This application is not registered with this server." };
    const redirectUri = params.get("redirect_uri") ?? (found.redirect_uris.length === 1 ? found.redirect_uris[0]! : "");
    if (!found.redirect_uris.some((uri) => sameRedirect(uri, redirectUri))) {
      return { error: "The application asked to return to an address it did not register." };
    }
    const fail = (error: string, description: string) => ({
      error: description,
      redirect: withParams(redirectUri, { error, error_description: description, state: params.get("state") ?? undefined, iss: origin(request) }),
    });
    if (params.get("response_type") !== "code") return fail("unsupported_response_type", "response_type must be code");
    const challenge = params.get("code_challenge") ?? "";
    if (params.get("code_challenge_method") !== "S256" || !/^[A-Za-z0-9_-]{43}$/.test(challenge)) {
      return fail("invalid_request", "PKCE is required: code_challenge with code_challenge_method S256");
    }
    const resource = params.get("resource") ?? resourceOf(request);
    if (resource.replace(/\/+$/, "") !== resourceOf(request)) {
      return fail("invalid_target", `resource must be ${resourceOf(request)}`);
    }
    return {
      client: found,
      pending: {
        clientId,
        redirectUri,
        state: params.get("state") ?? undefined,
        challenge,
        resource: resourceOf(request),
        expires: Date.now() + CONSENT_TTL * 1000,
      },
    };
  }

  async function authorize(request: Request): Promise<Response> {
    if (request.method === "GET") {
      const checked = await check(request, new URL(request.url).searchParams);
      if ("error" in checked) return checked.redirect ? redirect(checked.redirect) : errorPage(api, checked.error);
      return consentPage(api, {
        client: checked.client,
        redirectUri: checked.pending.redirectUri,
        pending: await keys.sign(JSON.stringify(checked.pending)),
        keysUrl: options.keysUrl,
      });
    }
    const form = await request.formData();
    const signed = await keys.verify(String(form.get("pending") ?? ""));
    const pending = signed ? (JSON.parse(signed) as Pending) : undefined;
    if (!pending || pending.expires < Date.now()) return errorPage(api, "This page expired: go back to the application and connect again.");
    const found = await client(pending.clientId);
    if (!found || !found.redirect_uris.some((uri) => sameRedirect(uri, pending.redirectUri))) {
      return errorPage(api, "This application is not registered with this server.");
    }
    if (form.get("action") !== "allow") {
      return redirect(withParams(pending.redirectUri, { error: "access_denied", state: pending.state, iss: origin(request) }));
    }
    const apiKey = String(form.get("api_key") ?? "").trim();
    const invalid = !apiKey ? "Enter your API key." : await options.validate?.(apiKey).catch(() => "The key could not be checked: try again.");
    if (invalid) {
      return consentPage(
        api,
        { client: found, redirectUri: pending.redirectUri, pending: String(form.get("pending")), keysUrl: options.keysUrl, error: invalid },
        400,
      );
    }
    const grantId = randomToken();
    const grant: Grant = { clientId: pending.clientId, key: await keys.encrypt(apiKey), expires: Date.now() + grantTtl * 1000 };
    await records.put(`grant:${grantId}`, grant, grantTtl);
    const code = randomToken();
    await records.put(
      `code:${await sha256(code)}`,
      {
        grantId,
        clientId: pending.clientId,
        redirectUri: pending.redirectUri,
        challenge: pending.challenge,
        resource: pending.resource,
      } satisfies Code,
      CODE_TTL,
    );
    return redirect(withParams(pending.redirectUri, { code, state: pending.state, iss: origin(request) }));
  }

  /** A token pair for a grant, neither outliving it. */
  async function issue(grantId: string, grant: Grant): Promise<Response> {
    const left = Math.floor((grant.expires - Date.now()) / 1000);
    if (left <= 0) return oauthError(400, "invalid_grant", "the grant has expired: connect again");
    const access = randomToken();
    const refresh = randomToken();
    const token: Token = { grantId, clientId: grant.clientId };
    await records.put(`access:${await sha256(access)}`, token, Math.min(accessTtl, left));
    await records.put(`refresh:${await sha256(refresh)}`, token, Math.min(refreshTtl, left));
    return json(200, { access_token: access, token_type: "Bearer", expires_in: Math.min(accessTtl, left), refresh_token: refresh });
  }

  async function token(request: Request): Promise<Response> {
    const form = await request.formData().catch(() => undefined);
    if (!form) return oauthError(400, "invalid_request", "the body must be application/x-www-form-urlencoded");
    const field = (name: string) => String(form.get(name) ?? "");
    const clientId = field("client_id");
    switch (field("grant_type")) {
      case "authorization_code": {
        const code = await records.take<Code>(`code:${await sha256(field("code"))}`);
        if (!code || code.clientId !== clientId || !sameRedirect(code.redirectUri, field("redirect_uri") || code.redirectUri)) {
          return oauthError(400, "invalid_grant", "the code is unknown, used, expired, or not this client's");
        }
        if ((await sha256(field("code_verifier"))) !== code.challenge) {
          return oauthError(400, "invalid_grant", "code_verifier does not match the code_challenge");
        }
        const resource = field("resource");
        if (resource && resource.replace(/\/+$/, "") !== code.resource) {
          return oauthError(400, "invalid_target", `resource must be ${code.resource}`);
        }
        const grant = await records.get<Grant>(`grant:${code.grantId}`);
        if (!grant) return oauthError(400, "invalid_grant", "the grant was revoked");
        return issue(code.grantId, grant);
      }
      case "refresh_token": {
        const hash = await sha256(field("refresh_token"));
        const refresh = await records.take<Token>(`refresh:${hash}`);
        if (!refresh) {
          // A refresh token used twice was stolen, or the client is confused: either way, its
          // grant ends (OAuth 2.1 §4.3.1).
          const reused = await records.get<string>(`used:${hash}`);
          if (reused) await records.delete(`grant:${reused}`);
          return oauthError(400, "invalid_grant", "the refresh token is unknown, used or expired");
        }
        await records.put(`used:${hash}`, refresh.grantId, refreshTtl);
        const grant = await records.get<Grant>(`grant:${refresh.grantId}`);
        if (refresh.clientId !== clientId || !grant) {
          return oauthError(400, "invalid_grant", "the refresh token is revoked, or not this client's");
        }
        return issue(refresh.grantId, grant);
      }
      default:
        return oauthError(400, "unsupported_grant_type", "grant_type: authorization_code or refresh_token");
    }
  }

  /** Revokes the grant behind a token (RFC 7009): every token of it stops working. */
  async function revoke(request: Request): Promise<Response> {
    const form = await request.formData().catch(() => undefined);
    const value = String(form?.get("token") ?? "");
    const hash = await sha256(value);
    const found = (await records.get<Token>(`access:${hash}`)) ?? (await records.get<Token>(`refresh:${hash}`));
    if (found) {
      await records.delete(`grant:${found.grantId}`);
      await records.delete(`access:${hash}`);
      await records.delete(`refresh:${hash}`);
    }
    return new Response(null, { status: 200 });
  }

  const unauthorized = (request: Request, description: string) =>
    json(
      401,
      { jsonrpc: "2.0", id: null, error: { code: -32001, message: description } },
      {
        "www-authenticate": `Bearer resource_metadata="${origin(request)}/.well-known/oauth-protected-resource", error="invalid_token", error_description="${description}"`,
      },
    );

  return {
    async route(request) {
      const path = new URL(request.url).pathname.replace(/\/+$/, "");
      const method = request.method;
      if (path === "/.well-known/oauth-protected-resource" || path === "/.well-known/oauth-protected-resource/mcp") {
        return json(200, resourceMetadata(request), { "access-control-allow-origin": "*" });
      }
      if (path === "/.well-known/oauth-authorization-server" || path === "/.well-known/openid-configuration") {
        return json(200, metadata(request), { "access-control-allow-origin": "*" });
      }
      if (method === "OPTIONS" && ["/register", "/token", "/revoke"].includes(path)) return cors(new Response(null, { status: 204 }));
      if (path === "/register" && method === "POST") return cors(await register(request));
      if (path === "/token" && method === "POST") return cors(await token(request));
      if (path === "/revoke" && method === "POST") return cors(await revoke(request));
      if (path === "/authorize" && (method === "GET" || method === "POST")) return authorize(request);
      return undefined;
    },

    async authenticate(request) {
      const value = /^Bearer\s+(.+)$/i.exec(request.headers.get("authorization") ?? "")?.[1]?.trim();
      if (!value) return unauthorized(request, "authorization required: connect through OAuth");
      const found = await records.get<Token>(`access:${await sha256(value)}`);
      const grant = found && (await records.get<Grant>(`grant:${found.grantId}`));
      if (!grant) return unauthorized(request, "the access token is unknown, expired or revoked");
      return { apiKey: await keys.decrypt(grant.key) };
    },
  };
}

/** A Client ID Metadata Document: its `client_id` is its own URL, its redirect URIs allowed ones.
 * Fetched without redirects, cut at 16 KB, never from a private address by name (and on Node,
 * where `fetch` is the guarded one of `perseid-mcp/node`, not by resolution either). */
async function clientMetadataDocument(url: string, fetchDocument: typeof fetch): Promise<Client | undefined> {
  if (url.length > 2048 || privateHost(new URL(url).hostname)) return undefined;
  try {
    const response = await fetchDocument(url, {
      headers: { accept: "application/json" },
      redirect: "error",
      signal: AbortSignal.timeout(5000),
    });
    if (!response.ok || !response.body) return undefined;
    const text = await readCapped(response.body, 16_384);
    if (text === undefined) return undefined;
    const document = JSON.parse(text) as { client_id?: unknown; client_name?: unknown; redirect_uris?: unknown };
    const uris = Array.isArray(document.redirect_uris) ? document.redirect_uris.filter((u): u is string => typeof u === "string") : [];
    if (document.client_id !== url || !uris.length || uris.length > 10 || !uris.every(redirectAllowed)) return undefined;
    return {
      client_id: url,
      client_name: typeof document.client_name === "string" ? document.client_name.slice(0, 100) : undefined,
      redirect_uris: uris,
      dynamic: false,
    };
  } catch {
    return undefined;
  }
}

/** A body as text, or nothing past `limit` bytes. */
async function readCapped(body: ReadableStream<Uint8Array>, limit: number): Promise<string | undefined> {
  const reader = body.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.length;
    if (size > limit) {
      await reader.cancel();
      return undefined;
    }
    chunks.push(value);
  }
  const bytes = new Uint8Array(size);
  let at = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, at);
    at += chunk.length;
  }
  return new TextDecoder().decode(bytes);
}

/** Hosts a metadata document must not be fetched from, by name: loopback, private, link-local
 * and reserved addresses, and names that are not public. */
export function privateHost(host: string): boolean {
  const name = host.replace(/^\[|\]$/g, "").replace(/\.+$/, "").toLowerCase();
  if (!name) return true;
  if (name.includes(":")) return privateAddress(name);
  if (/^[\d.]+$/.test(name) || /^0x/i.test(name)) return privateAddress(name);
  return (
    name === "localhost" ||
    !name.includes(".") ||
    /\.(localhost|local|internal|intranet|lan|home|corp|private|arpa|test|invalid|example)$/.test(name) ||
    /(^|\.)(nip|sslip|xip)\.io$/.test(name) ||
    name === "metadata.google.internal"
  );
}

/** Whether an IP address, v4 or v6, is not a public unicast one. Anything it cannot read as
 * dotted decimal IPv4 or IPv6 counts as private. */
export function privateAddress(address: string): boolean {
  const ip = address.replace(/^\[|\]$/g, "").toLowerCase();
  const v4 = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/.exec(ip)?.slice(1).map(Number);
  if (v4) {
    const [a, b] = v4 as [number, number];
    if (v4.some((part) => part > 255)) return true;
    return (
      a === 0 || a === 10 || a === 127 || a >= 224 ||
      (a === 100 && b >= 64 && b < 128) ||
      (a === 169 && b === 254) ||
      (a === 172 && b >= 16 && b < 32) ||
      (a === 192 && (b === 168 || b === 0)) ||
      (a === 198 && (b === 18 || b === 19))
    );
  }
  if (!ip.includes(":")) return true;
  // IPv4 inside IPv6 (::ffff:a.b.c.d, 64:ff9b::a.b.c.d, ::a.b.c.d): judged as that IPv4.
  const embedded = /(\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3})$/.exec(ip)?.[1];
  if (embedded) return privateAddress(embedded);
  const groups = expand(ip);
  if (!groups) return true;
  const [first] = groups as [number];
  if (groups.slice(0, 6).every((g) => g === 0)) return true; // ::, ::1, ::a.b.c.d in hex
  if (groups.slice(0, 5).every((g) => g === 0) && groups[5] === 0xffff) return true; // ::ffff:0:0/96
  if (first === 0x64 && groups[1] === 0xff9b) return true; // 64:ff9b::/96
  return (first & 0xfe00) === 0xfc00 || (first & 0xffc0) === 0xfe80 || (first & 0xffc0) === 0xfec0 || (first & 0xff00) === 0xff00 || first === 0x2001 && groups[1] === 0x0db8;
}

/** The eight groups of an IPv6 address, or nothing if it is not one. */
function expand(ip: string): number[] | undefined {
  const halves = ip.split("::");
  if (halves.length > 2) return undefined;
  const parse = (part: string) => (part ? part.split(":").map((g) => (/^[0-9a-f]{1,4}$/.test(g) ? parseInt(g, 16) : NaN)) : []);
  const head = parse(halves[0]!);
  const tail = halves.length === 2 ? parse(halves[1]!) : [];
  const missing = 8 - head.length - tail.length;
  if (halves.length === 1 ? head.length !== 8 : missing < 1) return undefined;
  const groups = [...head, ...Array(halves.length === 2 ? missing : 0).fill(0), ...tail];
  return groups.some(Number.isNaN) ? undefined : groups;
}

/** Redirect URIs: https, or http on a loopback address (native apps, RFC 8252). */
function redirectAllowed(uri: string): boolean {
  if (uri.length > 2048) return false;
  try {
    const url = new URL(uri);
    if (url.hash || url.username || url.password) return false;
    if (url.protocol === "https:") return true;
    return url.protocol === "http:" && ["localhost", "127.0.0.1", "[::1]"].includes(url.hostname);
  } catch {
    return false;
  }
}

/** The same redirect URI; on loopback, any port (RFC 8252). */
function sameRedirect(registered: string, requested: string): boolean {
  if (registered === requested) return true;
  try {
    const a = new URL(registered);
    const b = new URL(requested);
    const loopback = ["localhost", "127.0.0.1", "[::1]"];
    return (
      a.protocol === "http:" &&
      b.protocol === "http:" &&
      loopback.includes(a.hostname) &&
      a.hostname === b.hostname &&
      !b.username &&
      !b.password &&
      !b.hash &&
      a.pathname === b.pathname &&
      a.search === b.search
    );
  } catch {
    return false;
  }
}

function withParams(uri: string, params: Record<string, string | undefined>): string {
  const url = new URL(uri);
  for (const [name, value] of Object.entries(params)) if (value !== undefined) url.searchParams.set(name, value);
  return url.toString();
}

function redirect(location: string): Response {
  return new Response(null, { status: 302, headers: { location, "cache-control": "no-store" } });
}

function oauthError(status: number, error: string, description: string): Response {
  return json(status, { error, error_description: description });
}

/** Token, registration and revocation are called from browsers too (web clients). */
function cors(response: Response): Response {
  const headers = new Headers(response.headers);
  headers.set("access-control-allow-origin", "*");
  headers.set("access-control-allow-methods", "POST, OPTIONS");
  headers.set("access-control-allow-headers", "content-type, authorization");
  return new Response(response.body, { status: response.status, headers });
}

/** The address a request comes from, as the platform tells it: Cloudflare's header, else the
 * proxy's, else none (one bucket for all). */
function clientAddress(request: Request): string {
  return (
    request.headers.get("cf-connecting-ip") ??
    request.headers.get("x-forwarded-for")?.split(",")[0]?.trim() ??
    "unknown"
  );
}

/** At most `limit` events per key in `window` ms, in this process: a brake, not a quota. */
function rateLimit(limit: number, window: number): (key: string) => boolean {
  const events = new Map<string, number[]>();
  return (key) => {
    const now = Date.now();
    if (events.size > 10_000) events.clear();
    const recent = (events.get(key) ?? []).filter((time) => time > now - window);
    if (recent.length >= limit) return false;
    recent.push(now);
    events.set(key, recent);
    return true;
  };
}

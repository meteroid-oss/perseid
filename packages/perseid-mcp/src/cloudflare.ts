// The server as a Cloudflare Worker: Streamable HTTP on /mcp, the OAuth broker's grants in KV,
// scripts in QuickJS inside the Worker (any plan, no charge per script) or each in a Dynamic
// Worker of its own (Workers Paid, full V8, billed per script). Imports `cloudflare:workers`:
// for Workers only.
import wasmModule from "@jitl/quickjs-wasmfile-release-sync/wasm";
import release from "@jitl/quickjs-wasmfile-release-sync";
import { RpcTarget } from "cloudflare:workers";
import { newQuickJSWASMModuleFromVariant, newVariant, type QuickJSSyncVariant } from "quickjs-emscripten-core";

import { apiKeyAuth, createHandler, type Auth } from "./http.js";
import { oauthBroker } from "./oauth/broker.js";
import { resourceServer } from "./oauth/resource.js";
import { d1Store, kvStore } from "./oauth/store.js";
import { DEFAULT_LIMITS, limitConcurrency, PRELUDE, type Limits, type Message, type Sandbox, type Host } from "./sandbox/index.js";
import { quickjsSandbox } from "./sandbox/quickjs.js";
import type { Sources } from "./search.js";
import type { Api } from "./server.js";

/** The Worker's bindings and variables, all optional but those its auth needs. */
export interface Env {
  /** `api-key` (default): a key per request; `oauth`: the broker; `authorization-server`. */
  MCP_AUTH?: string;
  /** `quickjs` (default) or `dynamic-workers`, which needs `LOADER`. */
  MCP_SANDBOX?: string;
  MCP_READ_ONLY?: string;
  MCP_PUBLIC_URL?: string;
  MCP_TIMEOUT_MS?: string;
  MCP_MAX_CALLS?: string;
  /** oauth: a secret of 32 characters or more (`wrangler secret put MCP_OAUTH_SECRET`). */
  MCP_OAUTH_SECRET?: string;
  MCP_KEYS_URL?: string;
  /** oauth: the D1 database keeping grants, or else a KV namespace (eventually consistent: a code
   * raced from two locations may serve twice). */
  OAUTH_DB?: Parameters<typeof d1Store>[0];
  OAUTH_KV?: Parameters<typeof kvStore>[0];
  /** authorization-server: comma-separated URLs, and the introspection endpoint's credentials. */
  MCP_AUTHORIZATION_SERVERS?: string;
  MCP_INTROSPECTION_URL?: string;
  MCP_CLIENT_ID?: string;
  MCP_CLIENT_SECRET?: string;
  /** dynamic-workers: the Worker Loader binding. */
  LOADER?: WorkerLoader;
  [name: string]: unknown;
}

export interface WorkerOptions {
  /** The SDK's sources `search` quotes: `perseid-mcp sources` writes them as JSON at build. */
  sources?: Record<string, string>;
  /** oauth: checks a key on the consent page, by calling the API with it. */
  validate?: (apiKey: string, env: Env) => Promise<string | undefined>;
}

/** The Worker serving `api`: `export default mcpWorker(api, { sources })`. */
export function mcpWorker(api: Api, options: WorkerOptions = {}) {
  let cached: { env: Env; handler: (request: Request) => Promise<Response> } | undefined;
  return {
    async fetch(request: Request, env: Env): Promise<Response> {
      if (cached?.env !== env) cached = { env, handler: handler(api, env, options) };
      return cached.handler(request);
    },
  };
}

function handler(api: Api, env: Env, options: WorkerOptions) {
  const text = (name: string) => (typeof env[name] === "string" && env[name] ? (env[name] as string) : undefined);
  const number = (name: string, fallback: number) => {
    const value = Number(text(name));
    return Number.isInteger(value) && value > 0 ? value : fallback;
  };
  const required = (name: string, by: string) => {
    const value = text(name);
    if (!value) throw new Error(`MCP_AUTH=${by} needs ${name}`);
    return value;
  };
  const limits: Limits = {
    timeoutMs: number("MCP_TIMEOUT_MS", DEFAULT_LIMITS.timeoutMs),
    memoryMb: DEFAULT_LIMITS.memoryMb,
    calls: number("MCP_MAX_CALLS", DEFAULT_LIMITS.calls),
  };
  const sources: Sources = (file) => options.sources?.[file];
  let auth: Auth;
  switch (text("MCP_AUTH") ?? "api-key") {
    case "api-key":
      auth = apiKeyAuth(api);
      break;
    case "oauth":
      if (!env.OAUTH_DB && !env.OAUTH_KV) throw new Error("MCP_AUTH=oauth needs the OAUTH_DB binding, a D1 database");
      auth = oauthBroker(api, {
        store: env.OAUTH_DB ? d1Store(env.OAUTH_DB) : kvStore(env.OAUTH_KV!),
        secret: required("MCP_OAUTH_SECRET", "oauth"),
        url: text("MCP_PUBLIC_URL"),
        keysUrl: text("MCP_KEYS_URL"),
        validate: options.validate && ((key) => options.validate!(key, env)),
      });
      break;
    case "authorization-server":
      auth = resourceServer({
        authorizationServers: required("MCP_AUTHORIZATION_SERVERS", "authorization-server").split(",").map((s) => s.trim()),
        url: text("MCP_PUBLIC_URL"),
        introspection: text("MCP_INTROSPECTION_URL")
          ? {
              url: required("MCP_INTROSPECTION_URL", "authorization-server"),
              clientId: required("MCP_CLIENT_ID", "authorization-server"),
              clientSecret: required("MCP_CLIENT_SECRET", "authorization-server"),
            }
          : undefined,
      });
      break;
    default:
      throw new Error(`MCP_AUTH: expected api-key, oauth or authorization-server, got ${text("MCP_AUTH")}`);
  }
  let sandbox: Sandbox;
  switch (text("MCP_SANDBOX") ?? "quickjs") {
    case "quickjs":
      // Workers compile no WebAssembly at run time: the module is imported, compiled at deploy.
      const variant = newVariant(release as unknown as QuickJSSyncVariant, { wasmModule }) as QuickJSSyncVariant;
      // In the Worker's own isolate: a script computing holds this isolate's other requests, so few
      // run at once, and a loop is stopped by its count of checks (the clock stands still while
      // code runs on Workers), well within the Worker's CPU limit.
      sandbox = limitConcurrency(
        quickjsSandbox({ module: () => newQuickJSWASMModuleFromVariant(variant), cpuBudget: number("MCP_CPU_BUDGET", 10_000) }),
        4,
      );
      break;
    case "dynamic-workers":
      if (!env.LOADER) throw new Error("MCP_SANDBOX=dynamic-workers needs the LOADER binding, a Worker Loader");
      sandbox = limitConcurrency(dynamicWorkersSandbox(env.LOADER), 16);
      break;
    default:
      throw new Error(`MCP_SANDBOX: expected quickjs or dynamic-workers, got ${text("MCP_SANDBOX")}`);
  }
  return createHandler(api, {
    sandbox,
    sources,
    auth,
    baseURL: text(`${api.envPrefix}_BASE_URL`),
    readOnly: text("MCP_READ_ONLY") === "true",
    limits,
  });
}

/** The Worker Loader binding, as much of it as this module uses. */
export interface WorkerLoader {
  load(code: {
    compatibilityDate: string;
    mainModule: string;
    modules: Record<string, string>;
    globalOutbound: null;
    env?: Record<string, unknown>;
    limits?: { cpuMs?: number; subRequests?: number };
  }): { getEntrypoint(name?: string): { run(host: unknown): Promise<unknown> } };
}

/** Each script in a Dynamic Worker of its own: no network (`globalOutbound: null`), no bindings,
 * its only way out the host object it is handed, whose calls the session makes. Billed per
 * script: Cloudflare counts every `load()` as a Dynamic Worker created. */
export function dynamicWorkersSandbox(loader: WorkerLoader): Sandbox {
  return {
    async run(script: string, session: Host, limits: Limits): Promise<void> {
      const worker = loader.load({
        compatibilityDate: "2026-09-01",
        mainModule: "script.js",
        modules: { "script.js": ENTRYPOINT + PRELUDE + script },
        globalOutbound: null,
        env: {},
        limits: { cpuMs: Math.min(limits.timeoutMs, 30_000), subRequests: 0 },
      });
      let timer: ReturnType<typeof setTimeout> | undefined;
      const timeout = new Promise<void>((resolve) => {
        timer = setTimeout(() => {
          session.outcome.error ??= `the script ran longer than ${limits.timeoutMs} ms and was stopped`;
          resolve();
        }, limits.timeoutMs);
      });
      const run = worker
        .getEntrypoint("Script")
        .run(new HostTarget(session))
        .then(
          (end) => void session.handle(end as Message),
          (error: Error) => {
            session.outcome.error ??= /exceeded CPU/i.test(error.message)
              ? `the script ran longer than ${limits.timeoutMs} ms and was stopped`
              : error.message;
          },
        );
      await Promise.race([run, timeout]);
      clearTimeout(timer);
    },
  };
}

/** What the Dynamic Worker holds a stub of: the session's calls, nothing else. */
class HostTarget extends RpcTarget {
  constructor(private session: Host) {
    super();
  }
  async rpc(message: Message) {
    return this.session.handle(message);
  }
  async send(message: Message) {
    if (message.op === "log") await this.session.handle(message);
  }
}

/** `__rpc` and `__send` over the host stub; `__main`'s end is the entrypoint's return value. */
const ENTRYPOINT = String.raw`
import { WorkerEntrypoint } from "cloudflare:workers";
let __host;
let __id = 0;
const __logs = [];
function __send(message) {
  if (message.op === "log") __logs.push(__host.send(message));
}
function __rpc(op, payload) {
  return __host.rpc({ id: ++__id, op, ...payload }).then((reply) => {
    if (reply && reply.error) throw __error(reply.error);
    return reply ? reply.value : undefined;
  });
}
export class Script extends WorkerEntrypoint {
  async run(host) {
    __host = host;
    const end = await __main(__script);
    await Promise.all(__logs);
    return end;
  }
}
`;

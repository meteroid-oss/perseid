// `execute`: a script runs in a sandbox with no capability of its own: no network, files or
// environment. Its `client` is a proxy whose calls the host makes with the SDK, so credentials
// never reach the sandbox. The backends (Deno, QuickJS, Cloudflare's Dynamic Workers) only carry
// the messages of a `Session` between the script and the host.
import { transform } from "sucrase";

export interface Limits {
  /** Wall-clock time of a script. */
  timeoutMs: number;
  /** Memory of the sandbox, where the backend can cap it. */
  memoryMb: number;
  /** API calls a script may make, pages of a `for await` included. */
  calls: number;
}

export const DEFAULT_LIMITS: Limits = { timeoutMs: 30_000, memoryMb: 256, calls: 100 };

export interface Outcome {
  value?: unknown;
  error?: string;
  logs: string[];
  calls: number;
}

/** Refuses a call before the host makes it: read-only servers refuse writes. */
export type Guard = (accessor: string) => string | undefined;

/** Runs a script, JavaScript declaring `async function __script(client)`, carrying its messages
 * to `host`: the backend calls `__main(__script)` after it. */
export interface Sandbox {
  run(script: string, host: Host, limits: Limits): Promise<void>;
}

/** What a backend carries a script's messages to: a `Session`, or its proxy in another thread. */
export interface Host {
  readonly outcome: Outcome;
  handle(message: Message): Promise<Reply | undefined>;
  log(text: string): void;
}

/** `sandbox`, running at most `concurrency` scripts at once: the others wait their turn. */
export function limitConcurrency(sandbox: Sandbox, concurrency: number): Sandbox {
  let running = 0;
  const waiting: (() => void)[] = [];
  return {
    async run(script, host, limits) {
      if (running >= concurrency) await new Promise<void>((resolve) => waiting.push(resolve));
      running += 1;
      try {
        await sandbox.run(script, host, limits);
      } finally {
        running -= 1;
        waiting.shift()?.();
      }
    },
  };
}

/** A message from the script: a call for the host to make, a log, or the script's end. */
export type Message =
  | { id: number; op: "call" | "iterate"; path: string[]; args: unknown[] }
  | { id: number; op: "next"; handle: number }
  | { op: "log"; text: unknown }
  | { op: "done"; value: unknown }
  | { op: "error"; message: unknown };

/** The host's answer to a call: its value as JSON, or the error the script sees. */
export type Reply = { id: number; value: unknown } | { id: number; error: ScriptError };

export interface ScriptError {
  message: string;
  status?: number;
  body?: unknown;
}

/** Items a `for await` fetches from the host at a time. */
const BATCH = 100;

/** The largest message a backend carries from a script: a log line, a call, or its result. */
export const MAX_MESSAGE = 2 * 1024 * 1024;
/** What a script may log: each line cut at 2,000 characters, 64 KB in all. */
const MAX_LOG_LINE = 2000;
const MAX_LOGS = 64 * 1024;

/** What a session enforces beyond the sandbox's walls. */
export interface Policy {
  /** Refuses a call before the host makes it: operations only, writes on a read-only server. */
  guard?: Guard;
  /** Refuses any HTTP request but GET and HEAD, whatever the call. */
  readOnly?: boolean;
}

/** A client for one script, given the `fetch` its requests must go through. */
export type ClientFactory = (fetch: typeof globalThis.fetch) => object;

/** Runs `code`, the body of an async function of `client`, against a client in `sandbox`. */
export async function execute(
  code: string,
  client: object | ClientFactory,
  sandbox: Sandbox,
  limits: Limits = DEFAULT_LIMITS,
  policy: Policy | Guard = {},
): Promise<Outcome> {
  const session = new Session(client, limits, typeof policy === "function" ? { guard: policy } : policy);
  let script: string;
  try {
    script = compile(code);
  } catch (error) {
    session.outcome.error = `the script does not compile: ${(error as Error).message}`;
    return session.outcome;
  }
  try {
    await sandbox.run(script, session, limits);
  } finally {
    session.end();
  }
  if (session.outcome.error === undefined && !("value" in session.outcome)) {
    session.outcome.error = "the script ended without a result";
  }
  return session.outcome;
}

/** `code` as the JavaScript function `__script`, its TypeScript types stripped. */
export function compile(code: string): string {
  return transform(`async function __script(client) {\n${code}\n}`, {
    transforms: ["typescript"],
    disableESTransforms: true,
  }).code;
}

/** One script's calls, made with the SDK, and what it logged and returned. Every HTTP request
 * of its client goes through `fetch`, which counts it (pages and retries included), refuses
 * writes on a read-only server, and stops when the script ends. */
export class Session implements Host {
  readonly outcome: Outcome = { logs: [], calls: 0 };
  private iterators = new Map<number, { iterator: AsyncIterator<unknown>; busy: Promise<unknown> }>();
  private client: object;
  private guard: Guard;
  private requests = 0;
  private logged = 0;
  private ended = false;
  private abort = new AbortController();

  constructor(
    client: object | ClientFactory,
    private limits: Limits,
    private policy: Policy = {},
  ) {
    this.guard = policy.guard ?? (() => undefined);
    this.client = typeof client === "function" ? (client as ClientFactory)(this.fetch) : client;
  }

  /** Stops what the script started: requests in flight, retries, iterators. */
  end(): void {
    this.ended = true;
    this.abort.abort(new Error("the script has ended"));
  }

  /** The `fetch` of the script's client. */
  readonly fetch: typeof globalThis.fetch = async (input, init) => {
    if (this.ended) throw new Error("the script has ended");
    if (this.requests >= this.limits.calls) throw new Error(this.overLimit());
    this.requests += 1;
    const request = new Request(input, init);
    if (this.policy.readOnly && request.method !== "GET" && request.method !== "HEAD") {
      throw new Error(`${request.method} ${new URL(request.url).pathname}: this server is read-only`);
    }
    const headers = new Headers(request.headers);
    for (const name of METHOD_OVERRIDES) headers.delete(name);
    return globalThis.fetch(new Request(request, { headers, signal: AbortSignal.any([request.signal, this.abort.signal]) }));
  };

  /** Handles a message of the script: the reply to send back, for calls. */
  async handle(message: Message): Promise<Reply | undefined> {
    switch (message.op) {
      case "log":
        this.log(String(message.text));
        return undefined;
      case "done":
        this.outcome.value = message.value;
        return undefined;
      case "error":
        this.outcome.error = String(message.message).slice(0, MAX_LOG_LINE);
        return undefined;
      default:
        return this.call(message).then(
          (value) => ({ id: message.id, value }),
          (error) => ({ id: message.id, error: describe(error) }),
        );
    }
  }

  /** Keeps a line the script logged, within the budget. */
  log(text: string): void {
    if (this.logged >= MAX_LOGS) return;
    const line = text.length > MAX_LOG_LINE ? `${text.slice(0, MAX_LOG_LINE)}… (${text.length} characters)` : text;
    this.logged += line.length;
    this.outcome.logs.push(this.logged >= MAX_LOGS ? "… (more logs dropped)" : line);
  }

  private async call(message: Exclude<Message, { op: "log" | "done" | "error" }>): Promise<unknown> {
    if (this.ended) throw new Error("the script has ended");
    if (message.op === "next") {
      const entry = this.iterators.get(message.handle);
      if (!entry) throw new Error("unknown iterator");
      // One batch at a time per list, each counted before it is fetched.
      const batch = entry.busy.then(async () => {
        this.count();
        const items: unknown[] = [];
        for (let i = 0; i < BATCH; i++) {
          const step = await entry.iterator.next();
          if (step.done) return plain({ items, done: true });
          items.push(step.value);
        }
        return plain({ items, done: false });
      });
      entry.busy = batch.catch(() => undefined);
      return batch;
    }
    this.count();
    const refused = this.guard(message.path.join("."));
    if (refused) throw new Error(refused);
    const result = invoke(this.client, message.path, sanitize(message.args, this.limits));
    if (message.op === "call") return plain(await result);
    const iterator = (result as AsyncIterable<unknown>)[Symbol.asyncIterator]?.();
    if (!iterator) throw new Error(`client.${message.path.join(".")}() is not a list`);
    this.iterators.set(message.id, { iterator, busy: Promise.resolve() });
    return message.id;
  }

  private count(): void {
    if (this.outcome.calls >= this.limits.calls) throw new Error(this.overLimit());
    this.outcome.calls += 1;
  }

  private overLimit(): string {
    return `the script made more than ${this.limits.calls} API calls: ask for fewer items, or filter them`;
  }
}

/** Headers that would let a request act as another method than the one the guard checked. */
const METHOD_OVERRIDES = ["x-http-method-override", "x-http-method", "x-method-override"];

/** A call's arguments with the SDK's request options a script must not set: its own `fetch` or
 * `signal`, retries beyond two, a timeout beyond the script's own. */
function sanitize(args: unknown[], limits: Limits): unknown[] {
  const last = args[args.length - 1];
  if (!last || typeof last !== "object" || Array.isArray(last)) return args;
  const options = { ...(last as Record<string, unknown>) };
  delete options.fetch;
  delete options.signal;
  if (typeof options.maxRetries === "number") options.maxRetries = Math.max(0, Math.min(2, options.maxRetries));
  else if (options.maxRetries !== undefined) delete options.maxRetries;
  if (typeof options.timeout === "number") options.timeout = Math.max(0, Math.min(limits.timeoutMs, options.timeout));
  else if (options.timeout !== undefined) delete options.timeout;
  return [...args.slice(0, -1), options];
}

/** `client.things.attachments.upload(...args)`, by its path: never a private member (`_client`)
 * or an object's machinery (`constructor`, `__proto__`). */
function invoke(client: object, path: string[], args: unknown[]): unknown {
  if (!path.length || path.some((name) => name.startsWith("_") || name === "constructor" || name === "prototype")) {
    throw new Error(`client.${path.join(".")} is not a method: search for the operation first`);
  }
  let target: any = client;
  for (const name of path.slice(0, -1)) target = target?.[name];
  const method = target?.[path[path.length - 1]!];
  if (typeof method !== "function") {
    throw new Error(`client.${path.join(".")} is not a method: search for the operation first`);
  }
  return method.apply(target, args);
}

/** A result as JSON: a page as its body with `items` and `hasNextPage`, a file as its size,
 * type and base64 bytes. */
async function plain(value: unknown): Promise<unknown> {
  if (value && typeof value === "object" && typeof (value as any).hasNextPage === "function") {
    const page = value as { items: unknown[]; hasNextPage(): boolean };
    const body = JSON.parse(JSON.stringify(page));
    return { ...body, items: JSON.parse(JSON.stringify(page.items)), hasNextPage: page.hasNextPage() };
  }
  if (value && typeof value === "object" && typeof (value as any).arrayBuffer === "function") {
    const response = value as { arrayBuffer(): Promise<ArrayBuffer>; headers?: Headers };
    const bytes = new Uint8Array(await response.arrayBuffer());
    const contentType = response.headers?.get?.("content-type") ?? null;
    return { size: bytes.length, contentType, base64: bytes.length <= 1 << 20 ? base64(bytes) : null };
  }
  return value === undefined ? null : JSON.parse(JSON.stringify(value));
}

function base64(bytes: Uint8Array): string {
  let binary = "";
  for (let i = 0; i < bytes.length; i += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  }
  return btoa(binary);
}

/** An error as the script sees it: the API's status and body when it answered. */
function describe(error: unknown): ScriptError {
  const e = error as { message?: string; status?: number; body?: unknown };
  let body = e?.body;
  if (typeof body === "string") {
    try {
      body = JSON.parse(body);
    } catch {}
  }
  return { message: e?.message ?? String(error), status: e?.status, body };
}

/** Runs in the sandbox before the script: `client`, `console`, and `__main`. A backend defines
 * `__rpc(op, payload)`, a promise of the host's reply, and `__send(message)`, before it. */
export const PRELUDE = String.raw`
class __Call {
  constructor(path, args) { this.path = path; this.args = args; }
  then(resolve, reject) { return __rpc("call", { path: this.path, args: this.args }).then(resolve, reject); }
  catch(reject) { return this.then(undefined, reject); }
  [Symbol.asyncIterator]() {
    let handle, items = [], done = false;
    return {
      next: async () => {
        handle ??= await __rpc("iterate", { path: this.path, args: this.args });
        while (!items.length && !done) ({ items, done } = await __rpc("next", { handle }));
        return items.length ? { value: items.shift(), done: false } : { value: undefined, done: true };
      },
    };
  }
}
function __client(path) {
  return new Proxy(function () {}, {
    get: (_, name) => (name === "then" || typeof name === "symbol" ? undefined : __client([...path, name])),
    apply: (_, __, args) => new __Call(path, JSON.parse(JSON.stringify(args))),
  });
}
function __error(reply) {
  const error = new Error(reply.message);
  error.status = reply.status;
  error.body = reply.body;
  return error;
}
const __format = (value) => (typeof value === "string" ? value : JSON.stringify(value, null, 2));
globalThis.console = {};
for (const level of ["log", "info", "warn", "error", "debug"]) {
  console[level] = (...values) => __send({ op: "log", text: values.map(__format).join(" ") });
}
async function __main(run) {
  let end;
  try {
    const value = await run(__client([]));
    end = { op: "done", value: value === undefined ? null : JSON.parse(JSON.stringify(value)) };
  } catch (error) {
    const status = error?.status ? " (status " + error.status + ")" : "";
    end = { op: "error", message: (error?.message ?? String(error)) + status };
  }
  __send(end);
  if (typeof __end === "function") __end(end);
  return end;
}
`;

// The Deno backend: a process per script, with no permission at all, its messages a line of JSON
// each on stdin and stdout. Node only.
import { spawn } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

import { MAX_MESSAGE, PRELUDE, type Limits, type Message, type Sandbox, type Host } from "./index.js";

/** What a script may write to the host in all: calls, logs and its result. */
const MAX_OUTPUT = 32 * 1024 * 1024;

export interface DenoOptions {
  /** The Deno binary: `DENO_PATH`, else the `deno` npm package's, else the one on the PATH. */
  path?: string;
}

export function denoSandbox(options: DenoOptions = {}): Sandbox {
  const binary = options.path ?? deno();
  return {
    async run(script: string, session: Host, limits: Limits): Promise<void> {
      const dir = await mkdtemp(join(tmpdir(), "mcp-sandbox-"));
      const file = join(dir, "script.js");
      await writeFile(file, STDIO + PRELUDE + script + "\n__main(__script);\n");
      const child = spawn(
        binary,
        [
          "run",
          "--quiet",
          "--no-prompt",
          "--no-config",
          "--no-lock",
          "--no-remote",
          "--no-npm",
          `--v8-flags=--max-old-space-size=${limits.memoryMb}`,
          file,
        ],
        { env: { NO_COLOR: "1", DENO_DIR: join(dir, "cache"), HOME: dir }, stdio: ["pipe", "pipe", "pipe"] },
      );
      const send = (message: unknown) => {
        if (!child.stdin.destroyed) child.stdin.write(JSON.stringify(message) + "\n");
      };
      let stderr = "";
      let ended = false;
      const stop = (error: string) => {
        session.outcome.error ??= error;
        child.kill("SIGKILL");
      };
      child.stderr.on("data", (chunk) => (stderr = (stderr + chunk).slice(-4000)));
      child.stdin.on("error", () => {});
      // V8's heap limit leaves out ArrayBuffers: on Linux, the process's resident memory is
      // watched too, and the script stopped past its limit.
      const watch = setInterval(() => {
        const rss = residentMb(child.pid);
        if (rss !== undefined && rss > limits.memoryMb + 64) stop(`the script used more than ${limits.memoryMb} MB of memory`);
      }, 100);
      const finished = new Promise<void>((resolve) => {
        const timer = setTimeout(() => stop(`the script ran longer than ${limits.timeoutMs} ms and was stopped`), limits.timeoutMs);
        child.on("close", (status) => {
          clearTimeout(timer);
          clearInterval(watch);
          if (!ended && session.outcome.error === undefined) {
            session.outcome.error = stderr.trim() || `the sandbox exited with status ${status}`;
          }
          resolve();
        });
      });
      // Messages are lines of JSON; none longer than MAX_MESSAGE, nor more than MAX_OUTPUT in all.
      let buffer = "";
      let written = 0;
      const line = async (text: string) => {
        let message: Message;
        try {
          message = JSON.parse(text);
        } catch {
          session.log(text);
          return;
        }
        if (message.op === "done" || message.op === "error") {
          ended = true;
          child.stdin.end();
        }
        const reply = await session.handle(message);
        if (reply) send(reply);
      };
      child.stdout.setEncoding("utf8");
      child.stdout.on("data", (chunk: string) => {
        written += chunk.length;
        if (written > MAX_OUTPUT) return stop(`the script wrote more than ${MAX_OUTPUT} characters`);
        buffer += chunk;
        let end;
        while ((end = buffer.indexOf("\n")) >= 0) {
          void line(buffer.slice(0, end));
          buffer = buffer.slice(end + 1);
        }
        if (buffer.length > MAX_MESSAGE) stop(`the script sent a message longer than ${MAX_MESSAGE} characters`);
      });
      await finished;
      await rm(dir, { recursive: true, force: true });
    },
  };
}

/** The resident memory of a process in MB, where /proc tells it (Linux). */
function residentMb(pid: number | undefined): number | undefined {
  if (pid === undefined) return undefined;
  try {
    const kb = /VmRSS:\s+(\d+)/.exec(readFileSync(`/proc/${pid}/status`, "utf8"))?.[1];
    return kb === undefined ? undefined : Number(kb) / 1024;
  } catch {
    return undefined;
  }
}

/** Deno: `DENO_PATH`, the binary of the `deno` npm package, or the one on the PATH. */
function deno(): string {
  if (process.env.DENO_PATH) return process.env.DENO_PATH;
  try {
    const require = createRequire(import.meta.url);
    const binary = join(dirname(require.resolve("deno/package.json")), process.platform === "win32" ? "deno.exe" : "deno");
    if (existsSync(binary)) return binary;
    return require("deno/install_api.cjs").runInstall();
  } catch {
    return "deno";
  }
}

/** `__rpc` and `__send` over stdin and stdout. */
const STDIO = String.raw`
const __encoder = new TextEncoder();
const __pending = new Map();
let __id = 0;
function __send(message) {
  const bytes = __encoder.encode(JSON.stringify(message) + "\n");
  let written = 0;
  while (written < bytes.length) written += Deno.stdout.writeSync(bytes.subarray(written));
}
function __rpc(op, payload) {
  const id = ++__id;
  __send({ id, op, ...payload });
  return new Promise((resolve, reject) => __pending.set(id, { resolve, reject }));
}
function __end() {
  Deno.exit(0);
}
(async () => {
  let buffer = "";
  for await (const chunk of Deno.stdin.readable.pipeThrough(new TextDecoderStream())) {
    buffer += chunk;
    let end;
    while ((end = buffer.indexOf("\n")) >= 0) {
      const message = JSON.parse(buffer.slice(0, end));
      buffer = buffer.slice(end + 1);
      const waiting = __pending.get(message.id);
      __pending.delete(message.id);
      if (!waiting) continue;
      if (message.error) waiting.reject(__error(message.error));
      else waiting.resolve(message.value);
    }
  }
})();
`;

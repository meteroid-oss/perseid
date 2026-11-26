// `perseid-mcp runner`: runs scripts for MCP servers in another container (`remoteSandbox`), in
// Deno or QuickJS. It holds no API key and no grant: a script's calls go back to the server,
// which makes them. Node only.
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { timingSafeEqual } from "node:crypto";

import { MAX_MESSAGE, type Host, type Limits, type Message, type Outcome, type Reply, type Sandbox } from "./sandbox/index.js";

export interface RunnerOptions {
  port: number;
  host: string;
  token: string;
  sandbox: Sandbox;
}

export function serveRunner(options: RunnerOptions) {
  const expected = Buffer.from(`Bearer ${options.token}`);
  const server = createServer((request, response) => {
    if (request.url === "/health") return void response.writeHead(200).end("ok");
    const given = Buffer.from(request.headers.authorization ?? "");
    if (request.method !== "POST" || request.url !== "/run") return void response.writeHead(404).end();
    if (given.length !== expected.length || !timingSafeEqual(given, expected)) return void response.writeHead(401).end();
    void run(options.sandbox, request, response);
  });
  server.listen(options.port, options.host, () => {
    process.stderr.write(`perseid-mcp runner on http://${options.host}:${options.port}\n`);
  });
  return server;
}

async function run(sandbox: Sandbox, request: IncomingMessage, response: ServerResponse): Promise<void> {
  response.writeHead(200, { "content-type": "application/x-ndjson" });
  response.flushHeaders();
  const send = (value: unknown) => {
    if (!response.writableEnded) response.write(JSON.stringify(value) + "\n");
  };
  const pending = new Map<number, (reply: Reply | undefined) => void>();
  let seq = 0;
  let job: { script: string; limits: Limits } | undefined;
  const started = new Promise<void>((resolve, reject) => {
    let buffer = "";
    request.setEncoding("utf8");
    request.on("data", (chunk: string) => {
      buffer += chunk;
      if (buffer.length > MAX_MESSAGE * 2) {
        request.destroy();
        return reject(new Error("a message too long"));
      }
      let end;
      while ((end = buffer.indexOf("\n")) >= 0) {
        const line = JSON.parse(buffer.slice(0, end));
        buffer = buffer.slice(end + 1);
        if (!job) {
          job = line;
          resolve();
        } else {
          pending.get(line.seq)?.(line.reply);
          pending.delete(line.seq);
        }
      }
    });
    request.on("error", reject);
  });
  const outcome: Outcome = { logs: [], calls: 0 };
  try {
    await started;
    const host: Host = {
      outcome,
      log: (text) => send({ type: "message", message: { op: "log", text } }),
      async handle(message: Message) {
        if (message.op === "log" || message.op === "done" || message.op === "error") {
          send({ type: "message", message });
          return undefined;
        }
        const id = ++seq;
        send({ type: "call", seq: id, message });
        return new Promise((resolve) => pending.set(id, resolve));
      },
    };
    await sandbox.run(job!.script, host, job!.limits);
  } catch (error) {
    outcome.error ??= `the sandbox runner failed: ${(error as Error).message}`;
  }
  send({ type: "end", error: outcome.error });
  response.end();
}

// Scripts run in another container: `perseid-mcp runner`, which holds no key, no grant, and needs
// no network but to this server. Each script is one HTTP request: its body carries the script then
// the host's replies, its response the script's messages, a line of JSON each, both open at once.
// Node only.
import { request as httpRequest } from "node:http";

import { MAX_MESSAGE, type Host, type Limits, type Message, type Sandbox } from "./index.js";

export interface RemoteOptions {
  /** The runner's URL, `http://sandbox:8090`. */
  url: string;
  /** The token the runner requires, `MCP_RUNNER_TOKEN` on both sides. */
  token: string;
}

/** What the runner sends: a message of the script, a call awaiting a reply, its end. */
export type FromRunner =
  | { type: "message"; message: Message }
  | { type: "call"; seq: number; message: Message }
  | { type: "end"; error?: string };

export function remoteSandbox(options: RemoteOptions): Sandbox {
  const url = new URL("/run", options.url);
  return {
    run(script: string, host: Host, limits: Limits): Promise<void> {
      return new Promise<void>((resolve) => {
        let settled = false;
        const finish = (error?: string) => {
          if (settled) return;
          settled = true;
          if (error !== undefined) host.outcome.error ??= error;
          clearTimeout(timer);
          outgoing.destroy();
          resolve();
        };
        const timer = setTimeout(() => finish(`the script ran longer than ${limits.timeoutMs} ms and was stopped`), limits.timeoutMs + 2000);
        const outgoing = httpRequest(url, {
          method: "POST",
          headers: { authorization: `Bearer ${options.token}`, "content-type": "application/x-ndjson" },
        });
        const send = (value: unknown) => outgoing.write(JSON.stringify(value) + "\n");
        outgoing.on("error", (error) => finish(`the sandbox runner failed: ${error.message}`));
        outgoing.on("response", (response) => {
          if (response.statusCode !== 200) return finish(`the sandbox runner answered ${response.statusCode}`);
          let buffer = "";
          response.setEncoding("utf8");
          response.on("data", (chunk: string) => {
            buffer += chunk;
            if (buffer.length > MAX_MESSAGE * 2) return finish("the sandbox runner sent a message too long");
            let end;
            while ((end = buffer.indexOf("\n")) >= 0) {
              const event = JSON.parse(buffer.slice(0, end)) as FromRunner;
              buffer = buffer.slice(end + 1);
              if (event.type === "message") void host.handle(event.message);
              else if (event.type === "call") void host.handle(event.message).then((reply) => send({ seq: event.seq, reply }));
              else finish(event.error);
            }
          });
          response.on("end", () => finish("the sandbox runner closed the connection"));
        });
        send({ script, limits });
      });
    },
  };
}

// A worker thread of `quickjsThreadsSandbox`: runs scripts in QuickJS one at a time, its host the
// main thread, reached by messages.
import { parentPort } from "node:worker_threads";

import type { Host, Limits, Message, Outcome, Reply } from "./index.js";
import { quickjsSandbox } from "./quickjs.js";

const port = parentPort!;
const sandbox = quickjsSandbox();
const pending = new Map<number, (reply: Reply | undefined) => void>();
let seq = 0;

port.on("message", async (event: { type: "run"; script: string; limits: Limits } | { type: "reply"; seq: number; reply: Reply | undefined }) => {
  if (event.type === "reply") {
    pending.get(event.seq)?.(event.reply);
    pending.delete(event.seq);
    return;
  }
  const outcome: Outcome = { logs: [], calls: 0 };
  const host: Host = {
    outcome,
    log: (text) => port.postMessage({ type: "message", message: { op: "log", text } }),
    async handle(message: Message) {
      if (message.op === "log" || message.op === "done" || message.op === "error") {
        port.postMessage({ type: "message", message });
        return undefined;
      }
      const id = ++seq;
      port.postMessage({ type: "call", seq: id, message });
      return new Promise((resolve) => pending.set(id, resolve));
    },
  };
  try {
    await sandbox.run(event.script, host, event.limits);
  } catch (error) {
    outcome.error ??= `the sandbox failed: ${(error as Error).message}`;
  }
  pending.clear();
  port.postMessage({ type: "end", error: outcome.error });
});

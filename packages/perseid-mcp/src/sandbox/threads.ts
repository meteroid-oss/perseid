// QuickJS in worker threads, on Node: a script computing in a loop blocks its own thread, never
// the server's, and a script past its time is stopped by ending its thread. Each thread runs one
// script at a time; scripts beyond the pool's size wait their turn.
import { availableParallelism } from "node:os";
import { Worker } from "node:worker_threads";

import type { Host, Limits, Message, Sandbox } from "./index.js";

export interface ThreadsOptions {
  /** Threads, and so scripts running at once: the CPUs by default. */
  threads?: number;
}

/** What a thread sends: a message of the script, a call awaiting the host's reply, its end. */
type FromThread =
  | { type: "message"; message: Message }
  | { type: "call"; seq: number; message: Message }
  | { type: "end"; error?: string };

export function quickjsThreadsSandbox(options: ThreadsOptions = {}): Sandbox {
  const size = Math.max(1, options.threads ?? availableParallelism());
  const idle: Worker[] = [];
  let live = 0;
  const waiting: ((worker: Worker) => void)[] = [];
  const spawn = () => {
    live += 1;
    const worker = new Worker(new URL("./thread.js", import.meta.url));
    worker.unref();
    return worker;
  };
  const acquire = (): Promise<Worker> => {
    const worker = idle.pop();
    if (worker) return Promise.resolve(worker);
    if (live < size) return Promise.resolve(spawn());
    return new Promise((resolve) => waiting.push(resolve));
  };
  const release = (worker: Worker | undefined) => {
    const next = waiting.shift();
    if (next) next(worker ?? spawn());
    else if (worker) idle.push(worker);
  };
  return {
    async run(script: string, host: Host, limits: Limits): Promise<void> {
      const worker = await acquire();
      let healthy = true;
      try {
        await new Promise<void>((resolve) => {
          const done = () => {
            clearTimeout(timer);
            worker.off("message", onMessage);
            worker.off("error", onError);
            resolve();
          };
          const onMessage = async (event: FromThread) => {
            if (event.type === "message") await host.handle(event.message);
            else if (event.type === "call") worker.postMessage({ type: "reply", seq: event.seq, reply: await host.handle(event.message) });
            else {
              if (event.error !== undefined) host.outcome.error ??= event.error;
              done();
            }
          };
          const onError = (error: Error) => {
            healthy = false;
            host.outcome.error ??= `the sandbox failed: ${error.message}`;
            done();
          };
          // The thread enforces the time limit itself; past a grace period, it is ended.
          const timer = setTimeout(() => {
            healthy = false;
            host.outcome.error ??= `the script ran longer than ${limits.timeoutMs} ms and was stopped`;
            done();
          }, limits.timeoutMs + 1000);
          worker.on("message", onMessage);
          worker.on("error", onError);
          worker.postMessage({ type: "run", script, limits });
        });
      } finally {
        if (healthy) release(worker);
        else {
          live -= 1;
          void worker.terminate();
          release(undefined);
        }
      }
    },
  };
}

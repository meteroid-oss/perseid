// The QuickJS backend: QuickJS compiled to WebAssembly, in the host's process. A script gets a
// fresh runtime with nothing in it but what the prelude defines: no `fetch`, no timers, no I/O.
// Its only ways out are two host functions, `__host_rpc` and `__host_send`, carrying JSON.
import {
  newQuickJSWASMModuleFromVariant,
  shouldInterruptAfterDeadline,
  type QuickJSContext,
  type QuickJSWASMModule,
} from "quickjs-emscripten-core";

import { MAX_MESSAGE, PRELUDE, type Limits, type Message, type Sandbox, type Host } from "./index.js";

export interface QuickJSOptions {
  /** The WebAssembly build: by default `@jitl/quickjs-wasmfile-release-sync`. */
  module?: () => Promise<QuickJSWASMModule>;
  /** Stops a script after this many checks of the interpreter, whatever the clock says: about
   * 1.6 a millisecond of a tight loop. For Workers, whose clock stands still while code runs, so
   * that the time limit alone never stops a loop. */
  cpuBudget?: number;
}

export function quickjsSandbox(options: QuickJSOptions = {}): Sandbox {
  let module: Promise<QuickJSWASMModule> | undefined;
  const load = () =>
    (module ??= options.module?.() ??
      newQuickJSWASMModuleFromVariant(import("@jitl/quickjs-wasmfile-release-sync")));
  return {
    async run(script: string, session: Host, limits: Limits): Promise<void> {
      const quickjs = await load();
      const runtime = quickjs.newRuntime();
      const deadline = Date.now() + limits.timeoutMs;
      runtime.setMemoryLimit(limits.memoryMb * 1024 * 1024);
      runtime.setMaxStackSize(1024 * 1024);
      const late = shouldInterruptAfterDeadline(deadline);
      let checks = 0;
      let spent = false;
      runtime.setInterruptHandler((rt) => {
        if (options.cpuBudget !== undefined && ++checks > options.cpuBudget) spent = true;
        return spent || late(rt);
      });
      const context = runtime.newContext();
      let ended = false;
      let pending = 0;
      let wake: () => void = () => {};
      try {
        const tooLong = (text: string) => {
          if (text.length <= MAX_MESSAGE) return false;
          session.outcome.error ??= `the script sent a message longer than ${MAX_MESSAGE} characters`;
          ended = true;
          return true;
        };
        define(context, "__host_send", (text) => {
          if (tooLong(text)) return;
          const message = JSON.parse(text) as Message;
          if (message.op === "done" || message.op === "error") ended = true;
          if (message.op === "error" && message.message === "interrupted") {
            message.message = spent
              ? "the script computed for too long and was stopped: do less work in it"
              : `the script ran longer than ${limits.timeoutMs} ms and was stopped`;
          }
          void session.handle(message);
        });
        const rpc = context.newFunction("__host_rpc", (argument) => {
          const text = context.getString(argument);
          if (tooLong(text)) return context.undefined;
          const message = JSON.parse(text) as Message;
          const deferred = context.newPromise();
          pending += 1;
          session.handle(message).then((reply) => {
            pending -= 1;
            if (!context.alive) return;
            const text = context.newString(JSON.stringify(reply ?? null));
            deferred.resolve(text);
            text.dispose();
            wake();
          });
          return deferred.handle;
        });
        context.setProp(context.global, "__host_rpc", rpc);
        rpc.dispose();
        const result = context.evalCode(BRIDGE + PRELUDE + script + "\n__main(__script);\n", "script.js");
        if (result.error) {
          session.outcome.error = describe(context, result.error, limits, spent);
          result.error.dispose();
          return;
        }
        result.value.dispose();
        while (!ended) {
          const jobs = runtime.executePendingJobs();
          if (jobs.error) {
            session.outcome.error ??= describe(context, jobs.error, limits, spent);
            jobs.error.dispose();
            return;
          }
          if (ended) break;
          if (Date.now() > deadline) {
            session.outcome.error ??= `the script ran longer than ${limits.timeoutMs} ms and was stopped`;
            return;
          }
          if (pending === 0 && !runtime.hasPendingJob()) {
            session.outcome.error ??= "the script ended without a result: it awaits a promise that never settles";
            return;
          }
          if (!runtime.hasPendingJob()) {
            await new Promise<void>((resolve) => {
              const timer = setTimeout(resolve, Math.max(0, deadline - Date.now()) + 1);
              wake = () => {
                clearTimeout(timer);
                resolve();
              };
            });
          }
        }
      } finally {
        context.dispose();
        runtime.dispose();
      }
    },
  };
}

function define(context: QuickJSContext, name: string, fn: (text: string) => void): void {
  const handle = context.newFunction(name, (argument) => {
    fn(context.getString(argument));
  });
  context.setProp(context.global, name, handle);
  handle.dispose();
}

/** An exception of the interpreter: an interrupt is the time limit, the rest its message. */
function describe(context: QuickJSContext, error: Parameters<QuickJSContext["dump"]>[0], limits: Limits, spent: boolean): string {
  const dumped = context.dump(error) as { name?: string; message?: string } | string;
  const text = typeof dumped === "object" ? (dumped?.message ?? "") : String(dumped);
  if (/^interrupted$/.test(text)) {
    if (spent) return "the script computed for too long and was stopped: do less work in it";
    return `the script ran longer than ${limits.timeoutMs} ms and was stopped`;
  }
  if (/out of memory/.test(text)) {
    return `the script used more than ${limits.memoryMb} MB of memory`;
  }
  return typeof dumped === "object" ? `${dumped?.name ?? "Error"}: ${dumped?.message}` : String(dumped);
}

/** `__rpc` and `__send` over the host functions, in JSON. */
const BRIDGE = String.raw`
function __send(message) {
  __host_send(JSON.stringify(message));
}
let __id = 0;
function __rpc(op, payload) {
  return __host_rpc(JSON.stringify({ id: ++__id, op, ...payload })).then((text) => {
    const reply = JSON.parse(text);
    if (reply && reply.error) throw __error(reply.error);
    return reply ? reply.value : undefined;
  });
}
`;

// The multipart `_stream` twin of the SDK generated from tests/fixtures/edge-operations.yaml
// (`/transcriptions`, shaped as OpenAI's): the twin sends `stream=true` and reads the events, the
// other method leaves the part out, and neither takes it from the caller.
import assert from "node:assert/strict";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { after, describe, it } from "node:test";
import { generate, run, tsc } from "./_sdk.mjs";

const { sdkDir, sdk, cleanup } = await generate("edge-operations.yaml", { name: "TwinsSdk" });
after(cleanup);

/** A client answering events when the form asks for them, and the forms it received. */
function client() {
  const forms = [];
  const fetch = async (url, init) => {
    const form = await init.body.text();
    forms.push(form);
    return form.includes('name="stream"')
      ? new Response('data: {"id":"a"}\n\ndata: {"id":"b"}\n\n', { headers: { "content-type": "text/event-stream" } })
      : new Response('{"id":"a"}', { headers: { "content-type": "application/json" } });
  };
  return { forms, twins: new sdk.TwinsSdk({ apiKey: "tok", fetch, maxRetries: 0 }) };
}

describe("a multipart operation with a stream twin", () => {
  it("sends the stream part from the twin only", async () => {
    const { forms, twins } = client();
    const item = await twins.threads.createTranscription({ file: new Blob(["audio"]), language: "en" });
    assert.equal(item.id, "a");
    const ids = [];
    for await (const event of await twins.threads.createTranscriptionStream({ file: new Blob(["audio"]) })) {
      ids.push(event.id);
    }
    assert.deepEqual(ids, ["a", "b"]);
    assert.ok(!forms[0].includes('name="stream"'), forms[0]);
    assert.ok(forms[0].includes('name="language"\r\n\r\nen\r\n'), forms[0]);
    assert.match(forms[1], /name="stream"\r\n(.+\r\n)*\r\ntrue\r\n/);
  });

  it("does not take the stream part from the caller", async () => {
    writeFileSync(
      join(sdkDir, "src", "twinsProbe.ts"),
      `import { TwinsSdk } from "./index.js";

export async function probe(twins: TwinsSdk): Promise<void> {
  // @ts-expect-error the twins set the stream part
  await twins.threads.createTranscription({ file: new Blob([]), stream: true });
  // @ts-expect-error the twins set the stream part
  await twins.threads.createTranscriptionStream({ file: new Blob([]), stream: false });
}
`
    );
    await run(process.execPath, [tsc, "-p", ".", "--noEmit"], { cwd: sdkDir });
  });
});

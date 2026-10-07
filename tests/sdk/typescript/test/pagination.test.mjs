// The page of the SDK generated from tests/fixtures/edge-operations.yaml whose response has
// properties named like the paging members (`/paginated-clash`): the members win, on the page
// and in its type under `tsc --strict`, and the properties stay on `body`.
import assert from "node:assert/strict";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { after, describe, it } from "node:test";
import { generate, run, tsc } from "./_sdk.mjs";

const { sdkDir, sdk, cleanup } = await generate("edge-operations.yaml", { name: "EdgeSdk" });
after(cleanup);

const json = (body) => new Response(JSON.stringify(body), { headers: { "content-type": "application/json" } });

const CLASH = {
  data: [{ id: "a" }, { id: "b" }],
  meta: { total_pages: 2 },
  items: 7,
  body: "text",
  has_next_page: false,
  nextPage: "np",
  pages: ["p"],
};

/** A client answering with `bodies` in turn; `calls` records the requested URLs. */
function client(bodies) {
  const calls = [];
  const fetch = async (url) => {
    calls.push(new URL(url));
    return json(bodies[calls.length - 1]);
  };
  return { calls, edge: new sdk.EdgeSdk({ apiKey: "tok", fetch, maxRetries: 0 }) };
}

describe("a page whose response has properties named like its paging members", () => {
  it("reads the other properties on the page and the shadowed ones on its body", async () => {
    const { edge } = client([CLASH]);
    const page = await edge.flags.listPaginatedClash();
    assert.ok(page instanceof sdk.Page);
    assert.deepEqual(page.items.map((item) => item.id), ["a", "b"]);
    assert.deepEqual(page.data.map((item) => item.id), ["a", "b"]);
    assert.deepEqual(page.meta, { totalPages: 2 });
    assert.equal(page.nextPage, "np");
    assert.deepEqual(page.pages, ["p"]);
    assert.equal(typeof page.hasNextPage, "function");
    assert.equal(page.hasNextPage(), true);
    assert.equal(page.response.status, 200);
    assert.equal(page.body.items, 7);
    assert.equal(page.body.body, "text");
    assert.equal(page.body.hasNextPage, false);
    assert.deepEqual(JSON.parse(JSON.stringify(page)), JSON.parse(JSON.stringify(page.body)));
  });

  it("walks the pages from the first page number, and every item from the call", async () => {
    const second = { data: [{ id: "c" }], meta: { total_pages: 2 }, items: 8 };
    const { calls, edge } = client([CLASH, second, CLASH, second]);
    const first = await edge.flags.listPaginatedClash();
    const next = await first.getNextPage();
    assert.deepEqual(next.items.map((item) => item.id), ["c"]);
    assert.equal(next.body.items, 8);
    assert.equal(next.hasNextPage(), false);
    await assert.rejects(next.getNextPage(), sdk.EdgeSdkError);

    const ids = [];
    for await (const item of edge.flags.listPaginatedClash()) {
      ids.push(item.id);
    }
    assert.deepEqual(ids, ["a", "b", "c"]);
    assert.deepEqual(
      calls.map((url) => url.searchParams.get("page")),
      ["0", "1", "0", "1"]
    );
  });

  it("types the paging members over the properties they shadow", async () => {
    writeFileSync(
      join(sdkDir, "src", "pagingProbe.ts"),
      `import { EdgeSdk, type Item } from "./index.js";

type Equal<A, B> = (<X>() => X extends A ? 1 : 2) extends <X>() => X extends B ? 1 : 2 ? true : false;
const check = <T extends true>(_: T): void => undefined;

export async function probe(edge: EdgeSdk): Promise<void> {
  const page = await edge.flags.listPaginatedClash();
  check<Equal<typeof page.items, readonly Item[]>>(true);
  check<Equal<typeof page.body.items, number | undefined>>(true);
  check<Equal<typeof page.body.body, string | undefined>>(true);
  check<Equal<typeof page.hasNextPage, () => boolean>>(true);
  check<Equal<typeof page.data, Item[]>>(true);
  check<Equal<typeof page.nextPage, string | undefined>>(true);
  check<Equal<typeof page.pages, string[] | undefined>>(true);
  const totalPages: number | undefined = page.meta?.totalPages;
  const next = await page.getNextPage();
  check<Equal<typeof next.data, Item[]>>(true);
  for await (const each of page.iterPages()) {
    check<Equal<typeof each.data, Item[]>>(true);
  }
  for await (const item of edge.flags.listPaginatedClash()) {
    check<Equal<typeof item, Item>>(true);
  }
  void totalPages;
}
`
    );
    await run(process.execPath, [tsc, "-p", ".", "--noEmit"], { cwd: sdkDir });
  });
});

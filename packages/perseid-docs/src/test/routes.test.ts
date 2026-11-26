import { strict as assert } from "node:assert";
import { test } from "node:test";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { formatHref, loadRoutes, markdownHref, markdownPages, mintlifySlug, operationHref, groupHref, sidebar, staticPaths, webhookHref } from "../astro/routes.js";
import rehypeBaseLinks from "../astro/base-links.js";

test("a sub-resource keeps its root's URL and nests in the sidebar", () => {
  const op = (slug: string, title: string) => ({ id: slug, slug, title, method: "get", path: "/", auth: [], params: [], responses: [], sdk: {}, samples: {} });
  const reference = {
    title: "T", servers: [], auth: [], languages: [], models: {},
    groups: [
      { id: "add_ons", title: "Add-ons", operations: [op("a", "List add-ons")] },
      { id: "add_ons.entitlements", title: "Entitlements", parent: "add_ons", operations: [op("b", "List add-on entitlements")] },
    ],
  };
  const file = join(mkdtempSync(join(tmpdir(), "routes-")), "reference.json");
  writeFileSync(file, JSON.stringify(reference));
  const r = loadRoutes(file, "api-reference");
  const [root, sub] = r.reference.groups;
  assert.equal(operationHref(r, sub!.operations[0]!, sub!), "/api-reference/add-ons/list-add-on-entitlements/");
  assert.equal(groupHref(r, sub!), "/api-reference/add-ons/entitlements/");
  const top = sidebar(r) as { label: string; items?: { label: string }[] }[];
  assert.deepEqual(top.map((e) => e.label), ["Introduction", "Add-ons"]);
  assert.equal(top[1]!.items!.at(-1)!.label, "Entitlements");
  assert.equal(operationHref(r, root!.operations[0]!, root!), "/api-reference/add-ons/list-add-ons/");
});

test("routes load from the api folder the orchestrator writes", () => {
  const dir = mkdtempSync(join(tmpdir(), "api-"));
  const spec = {
    openapi: "3.1.0",
    info: { title: "Acme", version: "1" },
    paths: { "/things": { get: { operationId: "list_things", summary: "List things", tags: ["Things"], responses: { "200": { description: "ok" } } } } },
  };
  const data = {
    name: "Acme",
    base_url: "https://api.acme.com",
    resources: [{ name: "things", operations: ["list_things"] }],
    languages: {
      typescript: { version: "1.2.3", install: "npm install acme", setup: "const client = new Acme();", operations: {}, types: {} },
    },
  };
  writeFileSync(join(dir, "openapi.json"), JSON.stringify(spec));
  writeFileSync(join(dir, "docs-data.json"), JSON.stringify(data));
  const r = loadRoutes({ spec: join(dir, "openapi.json"), data: join(dir, "docs-data.json") }, "api-reference");
  assert.equal(r.reference.title, "Acme");
  assert.equal(r.reference.languages[0]?.version, "1.2.3");
  const group = r.reference.groups[0]!;
  assert.equal(operationHref(r, group.operations[0]!, group), "/api-reference/things/list-things/");
});

test("operations and webhooks keep the URLs Mintlify gave them", () => {
  const dir = mkdtempSync(join(tmpdir(), "mintlify-"));
  const ok = { "200": { description: "ok" } };
  const spec = {
    openapi: "3.1.0",
    info: { title: "Acme", version: "1" },
    paths: {
      "/v1/credit-units/{id}/prices": { get: { operationId: "list_prices", summary: "List a credit unit's selling prices", tags: ["Credit Units"], responses: ok } },
      "/api/v1/credit-notes/{credit_note_id}/download": { get: { operationId: "download_pdf", tags: ["Credit Notes"], responses: ok } },
      "/v1/things": {
        get: { operationId: "list_things", summary: "List things", tags: ["Things"], responses: ok },
        post: { operationId: "list_things_too", summary: "List things", tags: ["Things"], responses: ok },
      },
    },
    webhooks: {
      "addon.archived": { post: { summary: "addon.archived webhook", tags: ["webhooks"], responses: ok } },
      "credit_grant.expired": { post: { tags: ["webhooks"], responses: ok } },
    },
  };
  writeFileSync(join(dir, "openapi.json"), JSON.stringify(spec));
  const r = loadRoutes({ spec: join(dir, "openapi.json") }, "api-reference");
  const hrefs = r.reference.groups.flatMap((g) => g.operations.map((op) => operationHref(r, op, g)));
  assert.deepEqual(hrefs.sort(), [
    "/api-reference/credit-notes/get-apiv1credit-notes-download/",
    "/api-reference/credit-units/list-a-credit-units-selling-prices/",
    "/api-reference/things/list-things-1/",
    "/api-reference/things/list-things/",
  ]);
  assert.deepEqual(r.reference.webhooks!.map((w) => webhookHref(r, w)), [
    "/api-reference/webhooks/addonarchived-webhook/",
    "/api-reference/webhooks/credit_grantexpired/",
  ]);
  const slugs = staticPaths(r).map((p) => p.params.slug);
  assert.ok(slugs.includes("webhooks/addonarchived-webhook"));
  assert.equal(mintlifySlug("Get a license's activation code"), "get-a-licenses-activation-code");
});

test("hrefs follow the site's base and trailing slash; the sidebar leaves both to Starlight", () => {
  const dir = mkdtempSync(join(tmpdir(), "base-"));
  const spec = {
    openapi: "3.1.0",
    info: { title: "Acme", version: "1" },
    paths: { "/things": { get: { operationId: "list_things", summary: "List things", tags: ["Things"], responses: { "200": { description: "ok" } } } } },
  };
  writeFileSync(join(dir, "openapi.json"), JSON.stringify(spec));
  const format = { base: "/docs", trailingSlash: "never" as const };
  const r = loadRoutes({ spec: join(dir, "openapi.json") }, "api-reference", format);
  const group = r.reference.groups[0]!;
  assert.equal(operationHref(r, group.operations[0]!, group), "/docs/api-reference/things/list-things");
  assert.equal(groupHref(r, group), "/docs/api-reference/things");
  const items = JSON.stringify(sidebar(r));
  assert.ok(items.includes('"link":"/api-reference/things/list-things"'));
  assert.ok(!items.includes("/docs/"));
  const page = markdownPages(r).find((p) => p.path === "api-reference/things")!;
  assert.ok(page.text.includes("(/docs/api-reference/things/list-things.md)"));
  assert.equal(formatHref("", format), "/docs");
  assert.equal(formatHref("billing/plans", { base: "/", trailingSlash: "always" }), "/billing/plans/");
  assert.equal(markdownHref("", format), "/docs/index.md");
});

test("root-relative links of pages take the base, once", () => {
  const tree = {
    type: "root",
    children: [
      { type: "element", tagName: "a", properties: { href: "/billing/plans" } },
      { type: "element", tagName: "a", properties: { href: "/docs/billing" } },
      { type: "element", tagName: "a", properties: { href: "https://acme.com/x" } },
      { type: "element", tagName: "img", properties: { src: "/public/x.png", srcSet: "/a.png 1x, /b.png 2x" } },
      { type: "mdxJsxFlowElement", attributes: [{ type: "mdxJsxAttribute", name: "href", value: "/customers" }] },
    ],
  };
  rehypeBaseLinks({ base: "/docs/" })(tree);
  const [a, b, c, img, card] = tree.children as { properties?: Record<string, unknown>; attributes?: { value: unknown }[] }[];
  assert.equal(a!.properties!.href, "/docs/billing/plans");
  assert.equal(b!.properties!.href, "/docs/billing");
  assert.equal(c!.properties!.href, "https://acme.com/x");
  assert.equal(img!.properties!.src, "/docs/public/x.png");
  assert.equal(img!.properties!.srcSet, "/docs/a.png 1x, /docs/b.png 2x");
  assert.equal(card!.attributes![0]!.value, "/docs/customers");
});

import { strict as assert } from "node:assert";
import { test } from "node:test";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { loadRoutes, operationHref, groupHref, sidebar } from "../astro/routes.js";

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

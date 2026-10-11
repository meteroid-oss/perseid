import { strict as assert } from "node:assert";
import { test } from "node:test";
import { properties } from "../reference/schema.js";
import type { Reference } from "../reference/model.js";

const ref = {
  title: "T", servers: [], auth: [], languages: [], groups: [],
  models: {
    Order: { name: "Order", sdk: {}, schema: { type: "object", required: ["lines"], properties: { lines: { type: "array", items: { ref: "Line" } } } } },
    Line: { name: "Line", sdk: {}, schema: { type: "object", properties: { sku: { type: "string" }, parent: { ref: "Line" } } } },
  },
} as unknown as Reference;
const ctx = { ref, languages: [], modelHref: (n: string) => `#${n}` };

test("a list of models expands to the model's properties", () => {
  const html = properties({ ref: "Order" }, ctx);
  assert.match(html, /2 child attributes/);
  assert.match(html, /sku/);
});

test("a model nested in itself stops expanding", () => {
  const html = properties({ ref: "Line" }, ctx);
  assert.equal((html.match(/child attribute/g) ?? []).length, 0);
});

test("fields inherited through allOf keep the SDK's names", () => {
  const models = {
    Data: { name: "Data", sdk: { typescript: { name: "Data", fields: { created_at: "createdAt" } } }, schema: { type: "object", properties: { created_at: { type: "string" } } } },
    Event: { name: "Event", sdk: { typescript: { name: "Event", fields: { id: "id" } } }, schema: { allOf: [{ ref: "Data" }, { type: "object", properties: { id: { type: "string" } } }] } },
  };
  const withSdk = { ...ref, models } as unknown as Reference;
  const html = properties({ ref: "Event" }, { ref: withSdk, languages: ["typescript"], modelHref: (n: string) => `#${n}` });
  assert.match(html, /createdAt/);
});

import { strict as assert } from "node:assert";
import { test } from "node:test";
import { wrap } from "../build/wrap.js";
import { toSchema, type Obj } from "../build/spec.js";
import { assemble } from "../build/reference.js";
import { convertPage } from "../mintlify/import.js";

test("wrap hugs a lone object argument", () => {
  assert.equal(
    wrap('const customer = await client.customers.create({ currency: "EUR", name: "Acme Corporation" });', "typescript"),
    'const customer = await client.customers.create({\n  currency: "EUR",\n  name: "Acme Corporation",\n});',
  );
});

test("wrap leaves short lines and strings alone", () => {
  assert.equal(wrap('client.customers.retrieve("a, (b)")', "typescript"), 'client.customers.retrieve("a, (b)")');
});

test("wrap puts one builder call per line", () => {
  const out = wrap('var plan = client.plans().create(PlanCreateRequest.builder().name("Pro").currency("EUR").build());', "java");
  assert.match(out, /\n {8}\.name\("Pro"\)\n {8}\.currency\("EUR"\)\n/);
});

test("wrap keeps a single blank line after imports", () => {
  assert.equal(wrap('import os\n\nclient = Meteroid()', "python"), "import os\n\nclient = Meteroid()");
});

test("toSchema turns 3.1 null unions into nullable", () => {
  const spec = { openapi: "3.1.0" };
  assert.deepEqual(toSchema(spec, { type: ["string", "null"] }), { type: "string", nullable: true });
  assert.deepEqual(toSchema(spec, { oneOf: [{ $ref: "#/components/schemas/Address" }, { type: "null" }] }), {
    ref: "Address",
    nullable: true,
  });
});

test("convertPage maps frontmatter and imports the components used", () => {
  const out = convertPage('---\ntitle: Hi\nsidebarTitle: Short\n---\n<Note>x</Note>\n\n```\n<Card>\n```\n', []);
  assert.match(out, /^---\ntitle: Hi\nsidebar:\n {2}label: Short\n---/);
  assert.match(out, /import \{ Note \} from "perseid-docs\/mintlify";/);
});

test("convertPage reads a bare string frontmatter as the title", () => {
  assert.match(convertPage("---\nQuotes\n---\n\nBody\n", []), /^---\ntitle: Quotes\n---/);
});

test("wrap keeps imports whole and separates them from the call", () => {
  assert.equal(
    wrap('use acme::models::{CustomerCreateRequest, CustomTaxRate};\nlet c = client.customers().create(x).await?;', "rust"),
    "use acme::models::{CustomerCreateRequest, CustomTaxRate};\n\nlet c = client.customers().create(x).await?;",
  );
});

test("wrap leaves loop headers whole", () => {
  const loop = "for await (const customer of client.customers.list({ perPage: 100 })) {";
  assert.equal(wrap(loop, "typescript"), loop);
});

test("curl fills path parameters with the spec's examples", () => {
  const spec = {
    openapi: "3.1.0",
    info: { title: "Acme", version: "1" },
    paths: {
      "/things/{id}/{other}": {
        get: {
          operationId: "getThing",
          parameters: [
            { name: "id", in: "path", required: true, schema: { $ref: "#/components/schemas/ThingId" } },
            { name: "other", in: "path", required: true, schema: { type: "string" } },
          ],
          responses: { "200": { description: "ok" } },
        },
      },
    },
    components: { schemas: { ThingId: { type: "string", example: "thing_1" } } },
  };
  const curl = assemble(spec as unknown as Obj, undefined, { baseUrl: "https://x" }).groups[0]!.operations[0]!.samples.curl!;
  assert.match(curl, /"https:\/\/x\/things\/thing_1\/\{other\}"/);
});

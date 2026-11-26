# perseid-docs

Docs sites for APIs whose SDKs [perseid](https://github.com/meteroid-oss/perseid) generates. The
API reference follows the reader's SDK language: signatures, parameter and property names,
samples and return types change with it, in TypeScript, Python, Go, Java, C#, Rust and cURL.

```sh
# The docs model, from perseid.toml
npx perseid-docs build --out reference.json

# The reference as one HTML page
npx perseid-docs page reference.json --out index.html

# A Mintlify site, moved to an Astro project
npx perseid-docs import-mintlify ./docs --out site
```

A site is an Astro project with one integration:

```js
import { defineConfig } from "astro/config";
import perseidDocs from "perseid-docs";

export default defineConfig({
  integrations: [perseidDocs({ title: "Acme" })],
});
```

The API reference is built from `api/openapi.json` and `api/docs-data.json` (what
`perseid docs-data` writes), or from `reference: "reference.json"`.

## URLs

The reference's URLs are Mintlify's, so a site moved from Mintlify keeps its links and search
ranking: `/api-reference/<tag>/<summary>` for an operation (`<method>-<path>` without a summary)
and `/api-reference/webhooks/<summary>` for a webhook. `import-mintlify` also writes
`trailingSlash: "never"`, Mintlify's style.

Links follow Astro's `base` and `trailingSlash`, the reference's and the pages' alike, so a site
can live under a path of another, such as `https://www.acme.com/docs`:

```js
export default defineConfig({
  site: "https://www.acme.com",
  base: "/docs",
  trailingSlash: "never",
  integrations: [perseidDocs({ title: "Acme" })],
});
```

## A legacy version of the API

`references` adds API references beside the main one, each from its own folder or docs model. A
`legacy` one gets a banner on each page linking to the current reference. Without docs data, it
shows cURL:

```js
perseidDocs({
  title: "Acme",
  references: [{ id: "v1", label: "API v1", api: "api-v1", legacy: true }],
  topics: [
    { label: "Docs", link: "/", items: [/* ... */] },
    { label: "API Reference", link: "/api-reference", items: "reference" },
    { label: "API v1", link: "/api-reference/v1", items: "reference:v1" },
  ],
});
```

It lives at `/api-reference/<id>` unless `base` says otherwise.

Embedded, from a spec URL alone:

```html
<script src="https://cdn.jsdelivr.net/npm/perseid-docs/dist/embed.js"
        data-spec="https://api.acme.com/openapi.json" data-data="docs-data.json"></script>
```

`perseid-docs/reference` renders the reference to static HTML without a framework.

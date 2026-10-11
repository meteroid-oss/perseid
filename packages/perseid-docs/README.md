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

Embedded, from a spec URL alone:

```html
<script src="https://cdn.jsdelivr.net/npm/perseid-docs/dist/embed.js"
        data-spec="https://api.acme.com/openapi.json" data-data="docs-data.json"></script>
```

`perseid-docs/reference` renders the reference to static HTML without a framework.

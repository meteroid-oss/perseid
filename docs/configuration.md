# Configuration

`perseid init` writes a `perseid.toml` next to your spec:

```toml
spec = "openapi.json"               # or an https:// URL, JSON or YAML
name = "Acme"                       # Acme client, `acme` packages
base_url = "https://api.acme.com"

[rust]                              # generated into ./rust
[typescript]
package = "@acme/sdk"
[python]
[java]
package = "com.acme.sdk"
[csharp]                            # Acme namespace and NuGet package by default
[go]
module = "github.com/acme/acme-go"
repo = "acme/acme-go"               # lives in its own repository
```

## Top level

| Key | |
|---|---|
| `spec` | Path or URL of the OpenAPI document (3.0 or 3.1, JSON or YAML) |
| `name` | UpperCamelCase client name; package names derive from it |
| `base_url`, `header_prefix`, `user_agent`, `version`, `patch_nullable` | Defaults for every SDK |
| `webhooks` | Install the [webhook verifier](customizing.md#webhooks) |
| `include` | `only-public` (default, skips `x-internal: true`), `public-and-internal` or `only-internal` |
| `exclude`, `only` | Operation ids to leave out, or to keep exclusively |
| `overrides` | Directory of ejected templates and runtime, `.perseid` by default |
| `context` | Table exposed to templates as `sdk.*` |
| `[pagination]` | [Pagination rules](features.md#pagination) |

## Language tables

Every table takes `path`, `repo`, `package`, `version`, `base_url`, `header_prefix`, `user_agent`,
`webhooks`, `patch_nullable`, `exclude` (operation ids left out of that SDK only) and a `context` table. A table
with `repo = "owner/name"` generates into that repository, checked out under `.perseid/repos`.

Language-specific keys:

- `[go]`: `module`; `initialisms = true` spells names the Go way (`CustomerID`, `APIKey`);
  with `patch_nullable = true`, nullable optional PATCH fields are `*Nullable[T]`.
- `[typescript]`: `exports`, modules re-exported from the entry point; `int64` = `"number"`
  (default, exact up to 2^53), `"bigint"` or `"string"`.
- `[csharp]`: with `patch_nullable = true`, nullable optional PATCH fields are `MaybeUnset<T>`.
- `[python.context]`: `flat_unions = true` types unions as `Circle | Square` instead of a wrapper model.

`perseid init` turns on `initialisms` and `patch_nullable` for new SDKs; they rename or retype
public API, so existing SDKs opt in.

## Templates

`perseid inspect` prints the model templates receive. In templates,
`name | ident("snake", "rust")` turns a spec name into an identifier (cases `snake`, `camel`,
`pascal` and `shouty`, keywords escaped for `rust`, `python`, `go`, `java` or `typescript`), and
`names | idents(case, language, owner)` fails when two names collide.

## Spec support

- OpenAPI 3.0 documents are upgraded to 3.1 on load (`nullable`, boolean
  `exclusiveMinimum`/`exclusiveMaximum`). Swagger 2.0 is rejected: convert it first, for example
  with `npx swagger2openapi`.
- `$ref` parameters, bodies and responses, inline objects, `allOf` compositions, list bodies and
  `application/*+json` are read as is; inline objects become named types.
- A required `readOnly` property is optional in the models sent in requests, and a required
  `writeOnly` one in the models received in responses.
- Unsupported operations, and fields whose names would clash, make generation fail instead of
  being skipped, each listed with its operation id and path or its schema. Other schemas degrade
  with a warning: unions without a discriminator and schemas no SDK can model are typed as
  untyped JSON, enums whose values would share a name as strings, and schemas named like a type
  the SDK already uses (`Upload`, `Options`...) get a `Model` suffix.
- A variant missing from the discriminator `mapping` is tagged with the `const` or `enum` of its
  discriminator property, falling back to its schema name.

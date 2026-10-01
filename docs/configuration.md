# Configuration

`perseid init` writes a `perseid.toml` next to your spec:

```toml
spec = "openapi.json"               # or an https:// URL, JSON or YAML
name = "Acme"                       # Acme client, `acme` packages
base_url = "https://api.acme.com"
license = "MIT"                     # package metadata, from the spec and the git remote
repository = "https://github.com/acme/acme-sdks"

[rust]                              # generated into ./rust
[typescript]
package = "@acme/sdk"
[python]
[java]
package = "com.acme.sdk"
[csharp]                            # Acme namespace and NuGet package by default
[go]
repo = "acme/acme-go"               # lives in its own repository, module github.com/acme/acme-go
```

## Top level

| Key | |
|---|---|
| `spec` | Path or URL of the OpenAPI document (3.0 or 3.1, JSON or YAML), or `github:owner/repo/path` for a spec another repository pushes here (kept as `openapi.json`, or `.yaml` after its extension) |
| `repo` | Default repository of every SDK: `"acme/api-{lang}"` gives each its own (`{lang}` is `node`, `python`, `go`, `java`, `rust` or `dotnet`), `"acme/api-sdks"` holds them all, each in a folder named after its language. Without it, SDKs live here |
| `push_spec` | `owner/name` of a separate SDKs repository this repository sends its spec to, which generates the SDKs from its own `perseid.toml`; the SDK tables here only seed that file when `perseid setup` creates it. See [repository layouts](ci.md#repository-layouts) |
| `generate` | The command writing the spec in the API repository's CI when it isn't committed, run from its root before pushing the spec |
| `name` | UpperCamelCase client name; package names derive from it |
| `base_url`, `header_prefix`, `user_agent`, `version` | Defaults for every SDK |
| `webhooks` | Install the [webhook verifier](customizing.md#webhooks) |
| `[names]` | Method names by operation id, over the [resource-style names](#method-names) (as does `x-perseid-name` on an operation) |
| `timeout` | Default request timeout in seconds, 60 by default |
| `untagged_unions` | How unions of objects that no property tells apart decode: `json` (default, untyped JSON) or `best-match`; see [unions of objects](#unions-of-objects) |
| `license`, `repository`, `homepage`, `description`, `authors` | Package metadata written into the manifests `perseid init` creates |
| `include` | `only-public` (default, skips `x-internal: true`), `public-and-internal` or `only-internal` |
| `exclude`, `only` | Operation ids to leave out, or to keep exclusively |
| `overrides` | Directory of ejected templates and runtime, `.perseid` by default |
| `context` | Table exposed to templates as `sdk.*` |
| `[pagination]` | [Pagination rules](features.md#pagination) |

## Language tables

Every table takes `path`, `repo`, `package`, `version`, `base_url`, `header_prefix`, `user_agent`,
`webhooks`, `names`, `timeout`, `untagged_unions`, `exclude` (operation ids left out of that SDK
only) and a `context` table. A table with
`repo = "owner/name"` generates into that repository, checked out under `.perseid/repos`, at its
root, or in a folder named after the language when several SDKs share the repository; `path`
overrides either. Both override the top-level `repo`.

Language-specific keys:

- `[go]`: `module`.
- `[typescript]`: `exports`, modules re-exported from the entry point; `int64` = `"number"`
  (default, exact up to 2^53), `"bigint"` or `"string"`.
- `[python.context]`: `flat_unions = true` types unions as `Circle | Square` instead of a wrapper model.

`perseid init` fills the package metadata from the spec's `info` (license, contact,
description) and the `origin` git remote.

## Method names

Methods are named after the HTTP method and the path within their resource: `list`, `create`,
`retrieve`, `update`, `delete`, `list_sources`, `capture`. A name two operations of a resource
would share falls back to the operation id. `[names]` and `x-perseid-name` rename one.

## Unions of objects

A `oneOf`/`anyOf` of several objects without a `discriminator` (Stripe's
`Charge.customer: string | Customer | DeletedCustomer`) is typed when the
object variants can be told apart from their schemas: perseid looks for the fewest `const` or
single-value `enum` properties (`deleted: true`) and required properties only one variant
declares (`file_id`), and checks the most specific variant first. `DeletedCustomer` is picked
when `deleted` is `true`, else `Customer` when `object` is `"customer"`. The JSON type still
tells strings, numbers and lists apart.

When no such rule exists, the union is untyped JSON unless `untagged_unions = "best-match"`
(top level or a language table): an object is then the variant whose required properties are all
present and which has the most of its properties, the first declared one on ties. Generation
warns with the number of unions decoded that way, or left untyped. `x-perseid-union: best-match`
or `x-perseid-union: json` on the `oneOf`/`anyOf` schema overrides the setting for one union.

Either way, an object no variant matches, or that its variant cannot decode, is kept as received
in the union's unknown variant and serialized back unchanged, and every SDK can read the value as
another variant than the one picked (see [languages](languages.md)).

## Templates

`perseid inspect` prints the model templates receive. In templates,
`name | ident("snake", "rust")` turns a spec name into an identifier (cases `snake`, `camel`,
`pascal` and `shouty`, keywords escaped for `rust`, `python`, `go`, `java` or `typescript`), and
`names | idents(case, language, owner)` fails when two names collide. Descriptions are Markdown
(spec HTML is converted); the `doc` filter converts other text.

The model also carries, for templates to use:

- `op.errors`, the schema of each error response by status (`404`, `4XX`, `default`);
  `error_schemas` and `default_error` (the schema of nearly every operation's errors) on the API
  and in every template, and `is_error_schema` in type templates.
- Union field types (`is_union()`, `union_variants()`) for values of several types told apart by
  their JSON type, such as Stripe's expandable `string | Customer` or emptyable `object | ""`:
  each variant has a `name`, a `json_type`, `empty` for `""` and a `type`. They answer
  `is_json_object()` too, so templates not handling them keep untyped JSON; `union_refs` lists
  the schemas they reference. `union_mode()` is `json` when the JSON type decides, else `rules`
  (object variants have `when` conditions, `{property, value}` with no `value` for presence,
  checked in `object_variants()` order) or `best-match` (object variants list their `required`
  and all their `properties`). A struct variant with a string `id` has `id` set to `required` or
  `optional`.
- `type.discriminator_defaults`, the discriminator value of a struct that is a union variant.

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
  with a warning: unions without a discriminator whose variants share a JSON type (bar
  [unions of objects](#unions-of-objects)) and schemas no SDK can model are typed as untyped JSON, enums whose values would share a name as strings, and schemas named like a type
  the SDK already uses (`Upload`, `Options`...) get a `Model` suffix.
- A variant missing from the discriminator `mapping` is tagged with the `const` or `enum` of its
  discriminator property, falling back to its schema name.

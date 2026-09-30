# Configuration

`perseid init` writes a `perseid.toml` next to your spec:

```toml
spec = "openapi.json"               # or an https:// URL, JSON or YAML
name = "Acme"                       # Acme client, `acme` packages
base_url = "https://api.acme.com"
method_names = "resource"           # customers.list rather than customers.listCustomers
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
| `method_names` | `operation_id` (default): methods are named after operation ids; `resource`: after the HTTP method and the path within their resource (`list`, `create`, `retrieve`, `update`, `delete`, `list_sources`, `capture`), falling back to the operation id when two would collide |
| `[names]` | Method names by operation id, over `method_names` (as does `x-perseid-name` on an operation) |
| `timeout` | Default request timeout in seconds, 60 by default. SDKs generated before 0.4 used 15 s (none in TypeScript): set `timeout = 15` to keep that |
| `typed_unions` | Type values told apart by their JSON type, such as Stripe's expandable `string \| Customer` or `object \| ""`, in Rust, TypeScript, Python, Go and C# (Java: `edition = 2`), instead of untyped JSON; see [languages](languages.md) |
| `license`, `repository`, `homepage`, `description`, `authors` | Package metadata written into the manifests `perseid init` creates |
| `include` | `only-public` (default, skips `x-internal: true`), `public-and-internal` or `only-internal` |
| `exclude`, `only` | Operation ids to leave out, or to keep exclusively |
| `overrides` | Directory of ejected templates and runtime, `.perseid` by default |
| `context` | Table exposed to templates as `sdk.*` |
| `[pagination]` | [Pagination rules](features.md#pagination) |

## Language tables

Every table takes `path`, `repo`, `package`, `version`, `base_url`, `header_prefix`, `user_agent`,
`webhooks`, `patch_nullable`, `method_names`, `names`, `timeout`, `typed_unions`, `exclude`
(operation ids left out of that SDK only) and a `context` table. A table with
`repo = "owner/name"` generates into that repository, checked out under `.perseid/repos`.

Language-specific keys:

- `[go]`: `module`; `initialisms = true` spells names the Go way (`CustomerID`, `APIKey`);
  with `patch_nullable = true`, nullable optional PATCH fields are `*Nullable[T]`.
- `[typescript]`: `exports`, modules re-exported from the entry point; `int64` = `"number"`
  (default, exact up to 2^53), `"bigint"` or `"string"`.
- `[csharp]`: with `patch_nullable = true`, nullable optional PATCH fields are `MaybeUnset<T>`.
- `[java]`: `edition = 2` makes exceptions unchecked (no `throws IOException, ApiException`),
  enums classes that keep unknown values, types primitive-or-object unions as classes, and moves
  the HTTP client and `Utils` to an `internal` package. The scaffolded `ApiException` must extend `RuntimeException`.
- `[python.context]`: `flat_unions = true` types unions as `Circle | Square` instead of a wrapper model.

`perseid init` turns on `method_names = "resource"`, `initialisms`, `typed_unions`,
`patch_nullable` and Java's `edition = 2` for new SDKs; they rename or retype public API, so
existing SDKs opt in. It also fills the package metadata from the spec's `info` (license, contact,
description) and the `origin` git remote.

## Templates

`perseid inspect` prints the model templates receive. In templates,
`name | ident("snake", "rust")` turns a spec name into an identifier (cases `snake`, `camel`,
`pascal` and `shouty`, keywords escaped for `rust`, `python`, `go`, `java` or `typescript`), and
`names | idents(case, language, owner)` fails when two names collide. Descriptions are Markdown
(spec HTML is converted); the `doc` filter converts other text.

The model also carries, for templates to use:

- `op.method_name`, the resource-style name (`op.name` when `method_names = "resource"`).
- `op.errors`, the schema of each error response by status (`404`, `4XX`, `default`);
  `error_schemas` and `default_error` (the schema of nearly every operation's errors) on the API
  and in every template, and `is_error_schema` in type templates.
- Union field types (`is_union()`, `union_variants()`) for values of several types told apart by
  their JSON type, such as Stripe's expandable `string | Customer` or emptyable `object | ""`:
  each variant has a `name`, a `json_type`, `empty` for `""` and a `type`. They answer
  `is_json_object()` too, so templates not handling them keep untyped JSON; `union_refs` lists
  the schemas they reference.
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
  with a warning: unions without a discriminator whose variants share a JSON type and schemas no
  SDK can model are typed as untyped JSON, enums whose values would share a name as strings, and schemas named like a type
  the SDK already uses (`Upload`, `Options`...) get a `Model` suffix.
- A variant missing from the discriminator `mapping` is tagged with the `const` or `enum` of its
  discriminator property, falling back to its schema name.

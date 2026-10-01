# Configuration

`perseid init` writes a `perseid.toml` in the repository that will hold the SDKs. Every key,
annotated (only `name` and `sdks` are required):

```toml
#:schema https://raw.githubusercontent.com/meteroid-oss/perseid/main/perseid.schema.json
spec = "openapi.json"               # path or https:// URL (JSON or YAML), what generate reads
name = "Acme"                       # any case: "acme-api", "Acme API"... gives AcmeApi, acme_api, acme-api
sdks = ["typescript", "python", "go"] # among rust, typescript, python, go, java, csharp

# Where the SDKs live: here, in a folder per language, unless set
repo = "acme/acme-{lang}"           # one repository per SDK ({lang}: typescript, python, go...),
                                    # or "acme/acme-sdks": one repository, a folder per SDK
# release = false                   # no release-please files nor release workflow from setup

# Defaults of every SDK, each also settable in a language table
base_url = "https://api.acme.com"   # first server of the spec by default
timeout = 60                        # seconds
webhooks = false                    # install the Standard Webhooks verifier
untagged_unions = "json"            # or "best-match", see "Unions of objects"
header_prefix = "acme"              # SDK headers: acme-idempotency-key...; kebab-case name by default
user_agent = "acme"                 # User-Agent prefix; kebab-case name by default

# Operations
internal = false                    # true also generates operations marked x-internal: true
exclude = ["deleteEverything"]      # operation ids left out
# only = ["listWidgets"]            # or the only operation ids generated

overrides = ".perseid"              # ejected templates and runtime

[package]                           # written into the manifests `perseid generate` creates
description = "The Acme API client" # "{name} API client" by default
license = "MIT"                     # SPDX expression
homepage = "https://acme.com"
repository = "https://github.com/acme/acme-sdks"
authors = ["Acme <dev@acme.com>"]

[names]                             # method names by operation id
listWidgetEvents = "events"

[pagination]                        # or [[pagination]] for several rules
cursor = "starting_after"
item_cursor = "id"

[context]                           # exposed to templates as sdk.*
tagline = "widgets as a service"

[typescript]                        # overrides of a listed SDK
package = "@acme/sdk"
int64 = "bigint"
[python]
flat_unions = true
[go]
repo = "acme/acme-golang"           # its own repository name, over the top-level `repo`
```

## Editor completion

The first line points editors to [`perseid.schema.json`](../perseid.schema.json), a JSON Schema
(draft-07) of every key with its description and allowed values. VS Code's Even Better TOML,
IntelliJ and other [Taplo](https://taplo.tamasfe.dev)-based editors complete and check
`perseid.toml` with it; `taplo check perseid.toml` does it in CI.

## Spec, name and SDKs

| Key | |
|---|---|
| `spec` | The OpenAPI document (3.0 or 3.1, JSON or YAML) the SDKs are generated from: a path relative to `perseid.toml`, `openapi.json` by default, or an `http(s)` URL. When another repository holds the spec, `perseid connect` run there pushes it to this path, with `.perseid/source.json` naming its commit. `perseid generate --spec` reads another one |
| `name` | The client name, in any form: `acme-api`, `acme_api` and `Acme API` all give the `AcmeApi` client, the `acme_api` crate and Python package and the `acme-api` npm package. A name already in UpperCamelCase is kept as written (`AcmeAPI`) |
| `sdks` | The SDKs generated: `rust`, `typescript`, `python`, `go`, `java`, `csharp`. A [language table](#language-tables) only overrides the settings of a listed SDK |

## Where the SDKs live

See [repository layouts](ci.md#repository-layouts) for the trade-offs.

| Key | |
|---|---|
| `repo` | Default repository of every SDK: `"acme/api-{lang}"` gives each its own (`{lang}` is the language, as `sdks` names it), `"acme/api-sdks"` holds them all, each in a folder named after its language. Without it, SDKs live next to `perseid.toml` |
| `release` | `false` leaves out the release-please files and the `sdk-release.yml` workflow `perseid setup` adds to each repository holding SDKs |

## SDK defaults

Each is also a key of the [language tables](#language-tables), which override it for one SDK.

| Key | |
|---|---|
| `base_url` | The API base URL clients default to: the spec's first server, by `perseid init` |
| `timeout` | Default request timeout in seconds, 60 by default |
| `webhooks` | `true` installs the [webhook verifier](customizing.md#webhooks) |
| `untagged_unions` | How unions of objects that no property tells apart decode: `"json"` (default, untyped JSON) or `"best-match"`; see [unions of objects](#unions-of-objects) |
| `header_prefix` | Prefix of the headers the SDKs send on their own (`acme-idempotency-key`), the kebab-case `name` by default |
| `user_agent` | Prefix of the `User-Agent` header, the kebab-case `name` by default |
| `[names]` | Method names by operation id, over the [resource-style names](#method-names) (as does `x-perseid-name` on an operation) |
| `[context]` | Values exposed to templates as `sdk.*` |

## Operations

Every operation is generated, except those marked `x-internal: true`.

| Key | |
|---|---|
| `internal` | `true` also generates the `x-internal` operations |
| `exclude` | Operation ids left out of every SDK; a language table's `exclude` leaves some out of that SDK only |
| `only` | The operation ids generated, every other one left out (`x-internal` or not) |
| `[pagination]` | [Pagination rules](features.md#pagination), one table or an array of tables |

## Package metadata

`[package]` holds what the manifests `perseid generate` creates say about the packages. `init`
fills it from the spec's `info` (summary or description, license, contact) and the `origin` git
remote.

| Key | |
|---|---|
| `description` | One line, `"{name} API client"` by default |
| `license` | SPDX license expression |
| `homepage` | Project website |
| `repository` | URL of the source repository |
| `authors` | `"Name <email>"` of each author |

Versions aren't configured: each SDK's own manifest (`Cargo.toml`, `package.json`...) holds its
version, which [release-please](ci.md#releases) bumps.

## Language tables

`[rust]`, `[typescript]`, `[python]`, `[go]`, `[java]` and `[csharp]` each override the settings of
an SDK `sdks` lists, generated into a folder named after the language unless set otherwise. A
table for an SDK `sdks` doesn't list fails. Every table takes:

| Key | |
|---|---|
| `path` | Output directory, relative to the repository the SDK lives in |
| `repo` | `owner/name` of a GitHub repository to generate into, over the top-level `repo`: checked out under `.perseid/repos`, the SDK at its root, or in a folder named after the language when several SDKs share the repository |
| `package` | Crate, npm package, Python package, Go package, Java package or C# root namespace and NuGet package; derived from `name` by default |
| `base_url`, `timeout`, `webhooks`, `untagged_unions`, `header_prefix`, `user_agent`, `names`, `context` | Over the [SDK defaults](#sdk-defaults) |
| `exclude` | Operation ids left out of this SDK only |

And their own keys:

| Table | Key | |
|---|---|---|
| `[typescript]` | `exports` | Modules re-exported from the entry point |
| `[typescript]` | `int64` | TypeScript type of int64 values: `"number"` (default, exact up to 2^53), `"bigint"` or `"string"` |
| `[python]` | `flat_unions` | `true` types discriminated unions as `Circle \| Square` instead of a wrapper model |
| `[go]` | `module` | Module path, `github.com/{repo}/{path}` of the repository the SDK lives in by default |

## Renamed and removed keys

A key of an earlier version fails with what to write instead:

| Before | Now |
|---|---|
| `[push]`, `push_spec`, `sdks_repo`, `push_on`, `push_tags`, `generate` | `npx perseid connect <owner/sdks-repository>` in the repository holding the spec, whose `--on`, `--tags` and `--build` say when and how it pushes the spec |
| `spec = "github:acme/api/openapi.json"` | `spec = "openapi.json"`, where `perseid connect` pushes it |
| language tables alone | `sdks = [...]` listing the SDKs, the tables only overriding their settings |
| `node` and `dotnet` in `repo = "...-{lang}"` | `typescript` and `csharp`, or a `repo` in the language table |
| `description`, `license`, `homepage`, `repository`, `authors` | the same keys under `[package]` |
| `include = "public-and-internal"` | `internal = true` |
| `include = "only-internal"` | `only = [...]` listing them |
| `version`, at the top or in a language table | the SDK's own manifest, bumped by release-please |
| `[python.context] flat_unions = true` | `[python] flat_unions = true` |

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

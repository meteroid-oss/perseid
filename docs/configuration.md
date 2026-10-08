# Configuration

`perseid init` writes a `perseid.toml` in the repository that will hold the SDKs, or
`perseid init --from stainless.yml` from a [Stainless config](migrating-from-stainless.md). Every
key, annotated (only `name` and `sdks` are required):

```toml
#:schema https://raw.githubusercontent.com/meteroid-oss/perseid/main/perseid.schema.json
spec = "openapi.json"               # path or https:// URL (JSON or YAML), what generate reads
name = "Acme"                       # any case: "acme-api", "Acme API"... gives AcmeApi, acme_api, acme-api
sdks = ["typescript", "python", "go"] # among rust, typescript, python, go, java, csharp
idempotency_keys = false            # true when the API deduplicates POSTs by Idempotency-Key

# Where the SDKs live: here, in a folder per language, unless set
repo = "acme/acme-{lang}"           # one repository per SDK ({lang}: typescript, python, go...),
                                    # or "acme/acme-sdks": one repository, a folder per SDK
# release = false                   # no release-please files nor sdk-release.yml

# Defaults of every SDK, each also settable in a language table
base_url = "https://api.acme.com"   # first server of the spec by default
timeout = 60                        # seconds
webhooks = false                    # install the Standard Webhooks verifier
tests = true                        # generate a test per operation
round_trips = false                 # also decode and encode sample JSON of every model
untagged_unions = "best-match"       # or "json" for untyped JSON, see "Unions of objects"
header_prefix = "acme"              # SDK headers: acme-retry-count...; kebab-case name by default
user_agent = "acme"                 # User-Agent prefix; kebab-case name by default

# Operations
internal = false                    # true also generates operations marked x-internal: true
exclude = ["deleteEverything"]      # operation ids left out
# only = ["listWidgets"]            # or the only operation ids generated

overrides = ".perseid"              # ejected templates and runtime

[metadata]                          # written into the manifests `perseid generate` creates
description = "The Acme API client" # "{name} API client" by default
license = "MIT"                     # SPDX expression
homepage = "https://acme.com"
# repository = "https://git.acme.dev/api" # SDKs without `repo`: the `origin` remote by default
authors = ["Acme <dev@acme.com>"]

[types]                             # how values of some formats are typed, in every SDK
uuid = "typed"                      # or "string" for `format: uuid` values that are not all UUIDs

[methods]                           # method names by operation id
listWidgetEvents = "events"

[pagination]                        # or [[pagination]] for several rules
cursor = "starting_after"
item_cursor = "id"

[context]                           # exposed to templates as sdk.*
tagline = "widgets as a service"

[typescript]                        # overrides of a listed SDK
package = "@acme/sdk"
int64 = "bigint"
[go]
repo = "acme/acme-golang"           # its own repository name, over the top-level `repo`
```

## Editor completion

The first line points editors to [`perseid.schema.json`](../perseid.schema.json), a JSON Schema
of every key with its description and allowed values.

- VS Code's Even Better TOML, IntelliJ and other [Taplo](https://taplo.tamasfe.dev)-based
  editors complete and check `perseid.toml` with it.
- `taplo check perseid.toml` checks it in CI.

## Spec, name and SDKs

| Key | Description |
|---|---|
| `spec` | The OpenAPI document, a path relative to `perseid.toml` (`openapi.json` by default) or an `http(s)` URL |
| `name` | The client name, in any form |
| `sdks` | Among `rust`, `typescript`, `python`, `go`, `java`, `csharp` |
| `idempotency_keys` | `true` when the API deduplicates POST requests by `Idempotency-Key`: the SDKs send one with every POST and retry them. `false` by default: a POST is only retried when the caller gives it a key, as replaying it could apply it twice |

- The spec is Swagger 2.0 or OpenAPI 3.0, 3.1 or 3.2, JSON or YAML. `$ref`s to other files or URLs
  are bundled.
- When another repository holds the spec, `perseid connect` run there pushes it to the `spec`
  path, with `.perseid/source.json` naming its commit.
- `perseid generate --spec <path|url>` reads another spec.
- `acme-api`, `acme_api` and `Acme API` all give the `AcmeApi` client, the `acme_api` crate and
  Python package, and the `acme-api` npm package.
- A name already in UpperCamelCase is kept as written: `AcmeAPI`.

## Where the SDKs live

See [repository layouts](ci.md#repository-layouts) for the trade-offs.

| Key | Description |
|---|---|
| `repo` | `"acme/api-{lang}"`: a repository per SDK. `"acme/api-sdks"`: one repository, a folder per SDK. Unset: next to `perseid.toml` |
| `release` | `false` leaves out the release-please files and `sdk-release.yml` |
| `auto_merge` | `true` writes `auto-merge: true` into `sdks.yml`: SDKs whose tests pass are merged and released unattended. See [auto-merge](ci.md#auto-merge) |

`{lang}` is the language as `sdks` names it. `perseid init` writes the release files for SDKs
kept next to `perseid.toml`. The first pull request in each SDK repository carries them.

## SDK defaults

Each key is also a key of the [language tables](#language-tables), which override it for one SDK.

| Key | Default | Description |
|---|---|---|
| `base_url` | The spec's first server | API base URL of the clients |
| `timeout` | `60` | Request timeout, in seconds |
| `webhooks` | `false` | `true` installs the [webhook verifier](customizing.md#webhooks) |
| `tests` | `true` | `false` leaves out the [generated tests](languages.md#tests) |
| `round_trips` | `false` | `true` adds the [round trips](languages.md#round-trips) of every model to the generated tests |
| `untagged_unions` | `"best-match"` | `"json"` types [unions no property tells apart](#unions-of-objects) as untyped JSON |
| `header_prefix` | kebab-case `name` | Prefix of the headers the SDKs send on their own: `acme-retry-count` |
| `user_agent` | kebab-case `name` | Prefix of the `User-Agent` header |
| `[methods]` | | Method names by operation id, over the [resource-style names](#method-names) |
| `[context]` | | Values exposed to templates as `sdk.*` |

`[types]` holds settings of every SDK only:

| Key | Default | Description |
|---|---|---|
| `uuid` | `"typed"` | `format: uuid` values are the [language's UUID type](languages.md#formats). `"string"` keeps them strings, for an API whose "uuid" values are not all UUIDs: one would fail decoding the whole response |

## Operations

Every operation is generated, except those marked `x-internal: true`.

| Key | Description |
|---|---|
| `internal` | `true` also generates the `x-internal` operations |
| `exclude` | Operation ids left out of every SDK. In a language table, of that SDK only |
| `only` | The only operation ids generated, `x-internal` or not |
| `[pagination]` | [Pagination rules](features.md#pagination), one table or an array of tables |
| `detect_pagination` | `false` leaves unpaged the Stripe-style lists no rule matches, which perseid [detects](features.md#pagination) by default |

## Package metadata

`[metadata]` holds what the generated manifests say about the packages. `perseid init` fills it
from the spec's `info` and the license it asks for, and comments out the rest.

| Key | Description |
|---|---|
| `description` | One line, `"{name} API client"` by default |
| `license` | SPDX license expression |
| `homepage` | Project website |
| `repository` | Source repository URL of the SDKs without a `repo` |
| `authors` | `"Name <email>"` of each author |

- `perseid generate` writes the `license` text as each SDK's `LICENSE`, unless it has one: for
  `MIT`, `Apache-2.0`, `BSD-3-Clause`, `ISC`, `MPL-2.0` and `Unlicense`, the copyright line naming
  the `authors`, else `name`. Add it yourself for any other license.
- crates.io and Maven Central refuse packages without a license.
- Each manifest names its repository: `https://github.com/{repo}` for an SDK with a `repo`, else
  `repository`, else the `origin` remote.
- npm publishes with provenance, which needs `package.json` to name the publishing repository.
- Versions live in each SDK's own manifest (`Cargo.toml`, `package.json`...), bumped by
  [release-please](ci.md#releases).

## Language tables

`[rust]`, `[typescript]`, `[python]`, `[go]`, `[java]` and `[csharp]` override the settings of
an SDK that `sdks` lists. A table for an SDK not listed fails. `perseid init` writes one per SDK,
naming its package.

| Key | Description |
|---|---|
| `path` | Output directory, relative to the repository the SDK lives in. The language name by default |
| `repo` | `owner/name` of the repository to generate into, over the top-level `repo` |
| `package` | Crate, npm package, Python package, Go package, Java package, or C# root namespace and NuGet package. Derived from `name` by default |
| `exclude` | Operation ids left out of this SDK only |
| `base_url`, `timeout`, `webhooks`, `tests`, `round_trips`, `untagged_unions`, `header_prefix`, `user_agent`, `methods`, `context` | Over the [SDK defaults](#sdk-defaults) |

- An SDK with a `repo` is checked out under `.perseid/repos`, at its root, or in a folder named
  after the language when several SDKs share the repository.
- A `repo` naming the repository that holds `perseid.toml` (its `origin`) counts as no `repo`.

Keys of one language:

| Table | Key | Description |
|---|---|---|
| `[typescript]` | `exports` | Modules re-exported from the entry point |
| `[typescript]` | `int64` | Type of int64 values: `"number"` (default, exact up to 2^53), `"bigint"` or `"string"` |
| `[typescript]` | `validate_responses` | `false` skips checking response bodies against their schema (`true` by default) |
| `[go]` | `module` | Module path. `github.com/{repo}/{path}` by default |
| `[csharp.context]` | `dependency_injection` | `true` adds an `IHttpClientFactory` integration |

Without a `repo`, the Go module path comes from the `origin` remote or `[metadata] repository`.
Without either, `generate` warns and uses the kebab-case `name`.

## Method names

Methods are named after the HTTP method and the path within their resource.

| Operation | Method |
|---|---|
| `GET /customers`, `POST /customers` | `customers.list`, `customers.create` |
| `GET`, `POST` or `PATCH`, `DELETE /customers/{id}` | `retrieve`, `update`, `delete` |
| `GET /customers/search` | `search` |
| `GET /customers/{id}/sources` | `list_sources` |
| `POST /charges/{id}/capture` | `capture` |
| `archive_customer`, as `DELETE /customers/{id}` | `customers.archive` |
| `upload_file` | `files.upload` |
| `create_session`, as `POST /auth/session` | `create_session` |
| `GET /health` | `check`, `check_health` in another resource |

- When the path only says CRUD, an operation id starting with another verb names the method,
  without the resource's own noun.
- Resources are named after tags in snake_case. A single letter stays with the next word:
  `OAuth` is `oauth`, `IPAddress` is `ip_address`.
- A name two operations of a resource would share falls back to the operation id.
- `[methods]` or `x-perseid-name` on an operation renames one.

## Unions of objects

A `oneOf` is a tagged union when its object variants all require one property allowing a single,
distinct string (`kind: {enum: [pat]}`, as serde writes tagged enums). It behaves as if it
declared that property as `discriminator`.

- An `allOf` of such a union and objects gives every variant the objects' fields.
- A `oneOf` of one untagged object is that object.

Other `oneOf`/`anyOf` of several objects without a `discriminator`, such as Stripe's
`Charge.customer: string | Customer | DeletedCustomer`, are typed when their schemas tell the
variants apart:

1. perseid looks for the fewest `const` or single-value `enum` properties (`deleted: true`) and
   required properties only one variant declares (`file_id`).
2. The most specific variant is checked first: `DeletedCustomer` when `deleted` is `true`, else
   `Customer` when `object` is `"customer"`.
3. The JSON type tells strings, numbers and lists apart.
4. Variants sharing a JSON type (`string | string[] | integer[]`, `date-time | string`) are told
   apart by the type of a list's first item, else tried in order, plain `string` last.

When no rule tells two objects apart, a value is the variant whose required properties are all
present and which has the most of its properties, the first declared on ties.

| Setting | Effect |
|---|---|
| `untagged_unions = "best-match"` | Best match, as above (default) |
| `untagged_unions = "json"` | Untyped JSON |
| `x-perseid-union: best-match` or `json` on the schema | Overrides the setting for one union |

Generation warns with the number of unions decoded by best match, or left untyped. An object no
variant matches or decodes is kept as received and serialized back unchanged. Every SDK can read
a value as another variant than the one picked (see [languages](languages.md)).

## Spec support

| Spec | Handling |
|---|---|
| OpenAPI 3.0 | Upgraded to 3.1 on load (`nullable`, boolean `exclusiveMinimum`/`exclusiveMaximum`) |
| OpenAPI 3.2 | Read as 3.1. The `QUERY` method and `additionalOperations` are skipped with a warning |
| Swagger 2.0 | Converted to OpenAPI 3.0 in memory, with a note, then read as 3.0 (see below) |
| `$ref`s to other files or URLs | Bundled into `components.schemas` under names that collide with none. Escaped pointers (`~1`, `%7B`) resolve |
| Only `webhooks` (or `x-webhooks`) and components | Models, and a client without resources |

Swagger 2.0 becomes:

- `host`, `basePath` and `schemes`: a server per scheme, `https` first (the base URL), `https`
  when none is listed.
- `definitions`, `parameters` and `responses`: components, their `$ref`s following. Path items,
  parameters and responses in other files are inlined before the conversion, their schemas
  bundled as in 3.x.
- `body` parameters: a request body of each `consumes` media type (JSON by default).
  `formData` ones: a form body, or a multipart one when a field is a `type: file`. An
  operation's body or form replaces its path's body.
- `produces`: the media types of the responses; a `type: file` response is binary. An empty
  `consumes` or `produces` of an operation clears the global one.
- `collectionFormat`: `style` and `explode`, of form fields too (`tsv` is read as `csv`, with a
  warning).
- `securityDefinitions`: `basic` is HTTP basic, OAuth2 flows keep their URLs and scopes. An
  OAuth2 scheme without a known `flow` is an error.
- `x-nullable`: `nullable`. A string `discriminator`: its `propertyName`. Keywords next to a
  `$ref`, which 2.0 ignores, are dropped but for annotations (`description`, `readOnly`...).
- With `--bump auto`, oasdiff compares the conversions of 2.0 specs; the previous version's
  references to other files are read next to the current spec. A spec that fails to convert is
  compared as it is, with a warning.

Read as declared:

- `$ref` parameters, bodies and responses, cookie parameters, parameter `style`s and `content`.
- Inline objects, which become named types, and `allOf` compositions.
- List bodies, any JSON body, and `application/*+json`.
- `additionalProperties`: the typed map, or the unknown properties of the model, sent back.
- Nullable array items, map values and response bodies. Schemas recursive through an alias, a
  list or a map.
- A required `readOnly` property is optional in request models, a required `writeOnly` one in
  response models.

Discriminators and `allOf`:

- A discriminator on a base schema whose subtypes reference it in `allOf` makes the base a union
  of its subtypes: those of the `mapping`, plus those referencing the base or another subtype.
- The base's own fields move to `{Base}Base`. A subtype that other subtypes extend becomes the
  union of its own fields, as `{Subtype}Base`, and of its subtypes: a list of it decodes each
  one as its actual subtype.
- A variant missing from the `mapping` is tagged with its `x-ms-discriminator-value`, else the
  `const` or `enum` of its discriminator property, else its schema name. Variants sharing the
  same values, such as the `enum` of the base they inherit, take their schema name.
- An `allOf` of parts declaring the same property keeps the narrower schema.

Untyped JSON and skipped operations, each with a warning naming the operation or schema:

- Boolean schemas, `not` and tuple arrays (`prefixItems`) are untyped JSON.
- Unions without a discriminator whose variants share a JSON type, bar
  [unions of objects](#unions-of-objects), are untyped JSON.
- Unions Go cannot express are untyped JSON in Go only.
- Operations using an unsupported construct are skipped.
- `servers` of a path item or operation that the root `servers` do not list are ignored.

Error schemas:

- Each operation's own error schemas decode by status.
- A schema shared by at least three operations, by nine in ten of those declaring errors, and
  that is not mostly a success body, types every API error (`default_error`).
- One operation declaring `404: Item` never types the errors of the others.

## Names

Every SDK makes spec names valid identifiers:

| Spec | SDK |
|---|---|
| Schema names | UpperCamelCase types |
| A name starting with a digit | `N` prefix for types (`3DModel` is `N3dModel`), `value` prefix for identifiers |
| A type name the SDK or its runtime uses (`Upload`, `Options`, the client name, standard types) | `Model` suffix |
| Punctuation | Words: `created<` is `created_lt`, `-` is `minus` |
| Keywords of the language | Escaped: `r#type` in Rust, `lambda_` in Python, `final_` in Java |
| A name Go cannot export | `X` prefix |
| Enum values sharing an identifier | Numeric suffix: `bps` and `Bps` are `Bps` and `Bps2`. Repeated values are dropped |
| Parameters sharing a name | `_query`, `_header` or `_cookie` suffix |
| Method names that clash | The operation id |
| Tags | ASCII identifiers: `Petite Requête` is `petite_requete` |

Wire names are untouched. Serialized names with quotes or backslashes are escaped, and Go
properties a `json` tag cannot carry are encoded by hand.

Clashes perseid cannot resolve fail generation, all listed at once with where they are:

- two schemas becoming the same type (`a.b` and `AB`);
- two properties of a model becoming the same field (`type` and `@type`);
- two operations given the same method name by `x-perseid-name` or `[methods]`.

Rename one in the spec, or set `x-perseid-name` on an operation.

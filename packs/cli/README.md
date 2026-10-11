# perseid CLI pack

A command line for an API, generated from its spec on top of the Rust SDK perseid generates:
one clap command per operation, `acme customers list --limit 10`, `acme customers retrieve
cus_123`.

```toml
# perseid.toml, next to the spec
[targets.cli]                       # the built-in pack `cli`: this folder
wraps = "rust"
repo = "acme/acme-cli"
after = "rust"                      # once the Rust SDK is released

[context]
cli_client_id = "acme-cli"          # optional: the OAuth client of `auth login --web/--device`
cli_scopes = ["things:read"]        # optional: the scopes it asks for (default: the server's)
cli_revocation_url = "/oauth/revoke" # optional: RFC 7009 revocation on `auth logout`

[context.cli]                       # optional: the commands, see Generated code and Output
default_format = "yaml"             # on a terminal, instead of table
[context.cli.operations.customers_list_events]
hidden = true                       # out of help and completions, still a tool of `mcp`

[context.cli_mcp]                   # optional: the tools of `mcp`, see MCP server
instructions = "Amounts are in cents."
default_toolsets = ["billing"]
[context.cli_mcp.toolsets]
billing = ["subscriptions_*", "invoices_*", "archive_customer"]
admin = ["oauth_*", "batch_jobs_*"]
[context.cli_mcp.operations.customers_archive]
name = "archive_customer"
description = "Archive a customer: hidden from lists; its subscriptions keep running."
```

`perseid targets cli --out <dir>` previews it.

## The pack

```
packs/cli/
  pack.toml
  templates/      api_resource (a module per resource), api_summary (lib.rs, schema.json, Cargo.toml), macros, schema
  runtime/        the API-independent modules, vendored into generated/src/runtime
  runtime-crate/  the same modules as the crate perseid-cli-runtime: its tests and examples/acme.rs
  scaffold/       the binary crate and release automation, written once
  tests/          a fixture API, its mock server, golden --help and end-to-end tests
```

The runtime has one source, `runtime/`: perseid vendors each of its files, which carry the
`@generated` marker, and `runtime-crate/Cargo.toml` builds the same files as a crate with
`[lib] path = "../runtime/mod.rs"`, so CI tests them as they ship. The modules refer to each
other through `super::`, which works at any depth.

## The generated repository

```
Cargo.toml                    the binary crate `<name>-cli`, bin `<name>` (scaffold)
src/main.rs                   mounts the generated commands and yours, picks the login (scaffold)
generated/                    perseid's: the library crate `<name>-cli-generated`
  Cargo.toml                  the SDK (`sdk.version`) and the runtime's dependencies
  src/lib.rs                  NAME, ENV_PREFIX, DEFAULT_BASE_URL, DEFAULT_FORMAT, APP, AUTH, SCHEMA, command(), dispatch(), client(), From<sdk::Error>
  src/schema.json             the JSON Schema of the API's types, for `schema` and body checks
  src/commands/<resource>.rs  command() and dispatch() of a resource, a function per operation, its SCHEMA
  src/runtime/                the vendored runtime
dist-workspace.toml           dist: shell, PowerShell, Homebrew and npm installers (scaffold)
release-please-config.json    release-please, tags `v<version>` (scaffold)
.github/workflows/            ci.yml (fmt, clippy, test), release-please.yml, release.yml, publish-homebrew.yml, publish-npm.yml, publish-mcpb.yml (scaffold)
README.md, .gitignore         (scaffold)
```

`<name>` is the API's `name` in kebab case. The binary crate is the root package, and
`generated/` its path dependency rather than a workspace member, so release-please bumps one
version and every change of `generated/` counts towards the CLI's release. The scaffold's
README explains adding commands, logging in and releasing.

`release-please.yml` runs `meteroid-oss/perseid/release` as `sdk-release.yml` does, with
perseid's credentials (`SDK_APP_ID` and `SDK_APP_PRIVATE_KEY`, `SDK_GITHUB_TOKEN`, else the
hosted perseid App through OIDC, else the workflow token and `ci.yml` dispatched on the release
pull request). release-please drafts the GitHub Release and creates its tag (`draft`,
`force-tag-creation`), then the job dispatches the release workflow of dist on each released tag
with the workflow token (`dispatch-releases`, `create-release = false`): a tag pushed from a
workflow starts none, a dispatch does. dist builds, uploads, undrafts and publishes it.
`dist generate` regenerates `release.yml` after changing `dist-workspace.toml`.

Every installer is on: shell, PowerShell, a Homebrew formula and an npm package, both named
after the binary. Once the release is out, `publish-mcpb.yml` (`post-announce-jobs`) attaches an
MCP bundle per platform (`<binary>-darwin-arm64.mcpb`, …): the binary serving `mcp`, the API key
asked for at install, for Claude Desktop. dist has no way to make a publish job depend on a credential, so the scaffold
replaces its built-in `homebrew` and `npm` jobs by custom ones (`publish-jobs =
["./publish-homebrew", "./publish-npm"]`) running the same steps behind a check:

- Homebrew: a token for the tap, the `HOMEBREW_TAP` variable else `<owner>/homebrew-tap`, read
  at run time: of your App (`actions/create-github-app-token` for the tap alone), else the
  `HOMEBREW_TAP_TOKEN` or `SDK_GITHUB_TOKEN` secret, else of the perseid App, whose broker gives
  one for any repository of the installation holding the CLI's, through `hosted-token.sh` of
  perseid's `v0`.
- npm: trusted publishing, `npm publish --provenance` with the job's OIDC token (npm 11.5.1 or
  later, the trusted publisher naming `release.yml`, the calling workflow), the package's
  `repository` set to the CLI's. `NPM_TOKEN` only for the first publish, before the package
  exists on npm to trust the workflow; a version already there is skipped.

Without a credential the job notes so and succeeds, the release and its other installers go on.
The scaffold's README lists the steps; `release.yml` stays dist's own, so `dist generate
--check` and `dist plan` pass (`tests/run.sh` runs both when dist is installed).

## Generated code

Per operation of a resource, under its command (sub-resources nest: `plans versions list`):

- path parameters are positional arguments, `<THING_ID>`; array ones (`style: simple`) take
  `1,2,3`;
- query and header parameters are flags, typed: integers, numbers and booleans (`--flag`,
  `--flag=false`) are checked by clap, enums list their values (more than 12 are hidden from
  help), lists repeat the flag, and lists of enums or numbers also take `a,b,c` (`<STRING,...>`);
  numbers and enum values may start with `-`. Their text then goes through serde into the SDK's
  type (IDs, dates, decimals, enums), errors naming the flag and the expected form
  (`YYYY-MM-DD`);
- a JSON or form body takes `--data` and a flag per field of its schema: scalars, lists of
  scalars repeated, objects as JSON; multipart fields are flags, files `--<field> FILE|-`; a
  binary body is `--file FILE|-`. The body is checked against the schema before it is sent
  (see [Schemas](#schemas-and-validation)), then decoded into the SDK's request type;
- paginated operations fetch the page their page or cursor flag names (`--page 2`,
  `--starting-after th_2`) when given, else every page with `.items()`, stopping at
  `--max-items` (0 fetches nothing); on a terminal the first page only, with a hint, unless
  `--all`. Event streams print each event as it arrives, `jsonl` on a terminal too;
- JSON responses print with `ctx.print`; files need `--output FILE` (or `-o -` for stdout) and
  are checked before the request, then `{"path", "bytes", "content_type"}` prints; text goes to
  stdout or `--output`; empty responses print nothing;
- operations whose `security` is `[]` call the API without a credential;
- help is the first paragraph of the operation's description, its long help the whole
  description and the route; a parameter's or field's help is the first paragraph of its own
  (a field referring to an alias, such as `Email`, takes the alias's).
  Value names say what they take (`<DATE>`, `<DATE-TIME>`, `<UUID>`, `<DECIMAL>`, `<URI>`,
  `<EMAIL>` and other string formats, `<ID>`, `<INTEGER>`, `<JSON>`, `<FILE>`) and the bounds of
  numbers (`<INTEGER 1..100>`, `<NUMBER >0>`), required flags start with `(required)` and
  deprecated ones with `(deprecated)`, repeatable ones end with `(repeatable)`, those the API
  defaults with `[default: 20]` (only shown: the CLI does not send it), and a JSON flag shows
  the fields of its object, `*` marking the required ones: `{line1*, city, zip_code,
  country*}`, `{type*: online|bank_transfer, ...}`. The long help of an operation with a JSON
  body ends with an example, its required fields with the spec's examples, else defaults or
  placeholders within their bounds, and the `schema` command giving its body; that of an
  operation whose path or required parameters have examples in the spec ends with one.

Global options are listed in the root's help only; every other help ends with one line naming
them. A command's long help may say `{bin}` and `{words}`, which `app::command` replaces with
the binary's name and the command's words.

`[context.cli.operations.<tool>]` of perseid.toml, keyed like `[context.cli_mcp.operations]` by
the command's words joined by `_` before any rename (`customers list-events` is
`customers_list_events`), takes `hidden = true`: `app::command` leaves the command out of help,
completions and man pages, and `schema` out of its index, and its resource too when every command
of it is hidden. The command still runs when typed, and stays a tool of `mcp`, whose calls run it:
MCP has its own `hidden`.

Names are kebab case: `add-ons`, `list-entitlements`, `--per-page`. A top-level resource the
SDK named `<tag>_api` because a schema has its tag's name is `<tag>`: the model's
`display_path`, or, with perseid models older than it, the name without `_api` when a schema
has it. An operation repeating
its resource's name, plural or singular, after its verb loses it when the result clashes with
no other operation or sub-resource: `custom-properties list-custom-property-definitions` is
`custom-properties list-definitions`. Clashes are renamed deterministically:

| Clash | Becomes |
|---|---|
| a top-level resource named `auth`, `completion`, `man`, `schema`, `agents`, `mcp` or `help` | `auth-api`, ... |
| an operation or sub-resource named `help` | `help-api` |
| a sub-resource named like an operation of its parent | `<name>-api` |
| a path parameter named like a global flag (`format`, `jq`, `pretty`, `dry-run`, `all`, `allow-unknown-fields`, `profile`, `api-key`, `base-url`, `max-items`, `retries`, `no-retry`, `timeout`, `no-color`, `debug`), `data`, `output`, `help` or `version` | `path-<name>` |
| a query or header parameter so named, or named like a path parameter | `query-<name>`, `header-<name>` |
| a body field so named, or named like a parameter | `body-<name>` |

The client's builder gets `--retries`, `--no-retry` and `--timeout` (see
[Retries and timeouts](#retries-and-timeouts)), and under `--debug` a middleware logging each
HTTP attempt.

`From<sdk::error::Error> for CliError` maps API errors to exit code 3 (4xx) or 4 (5xx) with
their body, request id and `Retry-After`, timeouts and connection failures to 5, decoding
failures to 1, and invalid requests to 2.

Besides `src/lib.rs` and `Cargo.toml`, the summary template writes `src/schema.json`: the JSON
Schema of every type of the model and the command words of each resource. Each resource module
has a `SCHEMA` constant, the JSON of its commands (method, path, summary, parameters with their
argument, body, response, pagination, an example body); `lib.rs` gathers them all in
`SCHEMA`, which `APP` hands the runtime. The client gets an `HttpClient` recording the request
under `--dry-run` (see [Dry runs](#dry-runs)).

### Logging in

`generated::AUTH` is what the spec's security schemes allow:

- `auth::ApiKey`, the default: bearer and API key schemes take the key `auth login` prompts
  for or reads from stdin, or `<PREFIX>_API_KEY`;
- with an `oauth2` scheme declaring the `authorizationCode` flow and, as OpenAPI 3.2 does,
  `deviceAuthorization`, and `cli_client_id` set in perseid.toml's `[context]`: `auth::Methods`,
  the API key by default, `auth login --web` (`oauth::Browser`, PKCE) and `--device`
  (`oauth::DeviceCode`), with the flows' URLs as the spec writes them. Without `cli_client_id`
  it stays `ApiKey`, its doc comment saying so.

The runtime resolves relative OAuth URLs when it uses them, against the base URL per RFC 3986
(`/oauth/token` against `https://api.acme.dev/v1` is `https://api.acme.dev/oauth/token`), as
OpenAPI resolves them against its server: `--base-url`, `<PREFIX>_BASE_URL` or the profile's
move them with the API, and `DEFAULT_BASE_URL` (perseid.toml's `base_url`) applies otherwise.
Absolute URLs stay as they are.

Logins ask for no scope, leaving the authorization server's default, unless `cli_scopes` of
`[context]` lists some (a list, or a space-separated string): the declared scopes of a flow are
every scope the server knows, not what the CLI needs. OpenAPI has no revocation endpoint, so
`cli_revocation_url` gives it, absolute or relative like the flows' URLs; without it `auth
logout` only forgets the tokens.

The scaffold's `src/main.rs` owns the choice: `App { auth: &generated::AUTH, ..generated::APP }`,
which a custom `Auth` replaces (see [Authentication](#authentication)).

## The runtime's contract

[`runtime-crate/examples/acme.rs`](runtime-crate/examples/acme.rs) is the reference generated
code follows, by hand: a root command, a paginated `things list` with a query flag, and `things
create` with `--data` and field flags, against a fake SDK shaped like
[meteroid-rs](https://github.com/meteroid-oss/meteroid-rust). `runtime-crate/tests/acme.rs` runs
it end to end.

| Generated code calls | For |
|---|---|
| `app::command(&APP, root)` | the root with global options and built-in commands |
| `app::run(APP, command(), dispatch)` | `main`: parsing, built-ins, credentials, Tokio, errors, exit code; `dispatch` is an `Fn`, which `mcp` calls once per tool call |
| `ctx.token().await?`, `ctx.base_url()` | the bearer token (refreshed if needed) and base URL, for the SDK's builder |
| `ctx.max_retries()`, `ctx.timeout()`, `ctx.attempts()` | `--retries`, `--timeout` and `--debug`'s log of attempts, for the SDK's builder |
| `App { default_base_url: DEFAULT_BASE_URL, .. }`, `ctx.api_base_url()` | the SDK's default base URL, which OAuth URLs resolve against without `--base-url` |
| `App { default_format: DEFAULT_FORMAT, .. }` | the format on a terminal without `--format`, `None` for `table` |
| `App { schema: SCHEMA, .. }` | the JSON Schema parts `schema` prints and bodies are checked against |
| `ctx.print(&response)?` | one response, in the chosen format, through `--jq` |
| `ctx.items()`, `is_full()`, `first_page_only()`, `fetched_page(more, chosen)`, `push(&item)?`, `finish()?` | paginated items, streamed; one page on a terminal |
| `ctx.events()` then `push` and `finish` | the events of a stream |
| `args::args(PARAMS)`, `args::one`, `many`, `required`, `positional`, `positionals`, `given` | path, query and header parameters, as the SDK's types |
| `args::file`, `args::files`, `args::output_path`, `args::Download`, `args::write_output` | uploads, file downloads streamed as their chunks arrive, text responses |
| `body::args(FIELDS)`, `Body::from_matches(m, FIELDS, ctx.stdin())?` | `--data` and field flags |
| `body.validate(ctx, "things.create", optional)?` | the body checked against the command's schema |
| `body.decode::<T>()?`, `body.decode_optional::<T>()?` | the SDK's request type, errors naming the flag |
| `ctx.dry_run()`, `dry_run::Recorder::record(request)` | `--dry-run`: the SDK's `HttpClient` recording the request |
| `CliError::{api, timeout, network, decode, invalid_request, usage, internal}`, `with_retry_after` | `From<sdk::Error>` |
| `app::run_with(APP, command(), args, Host::captured(stdin), dispatch)` | tests, in process |
| `mcp::check(&APP)` | tests: the names `[context.cli_mcp]` gives tools |

Generated resources must not take the name of a top-level built-in (`app::BUILTINS`: `auth`,
`completion`, `man`, `schema`, `agents`, `mcp`).

## Behavior

### Output

`--format json|jsonl|yaml|table`, else `<PREFIX>_FORMAT`. Without either, on a terminal:
`table`, or the `default_format` of perseid.toml's `[context.cli]` (`DEFAULT_FORMAT`, which an
unknown value makes a `compile_error!`); off a terminal: `json` for one response, `jsonl` for
paginated lists and event streams (one item per line, printed as they arrive), whatever
`default_format` says, as programs read it. Event streams are `jsonl` on a terminal too.

- JSON is compact off a terminal, one line per document, and indented on one or with
  `--pretty`. Tables alone have colors: `json` and `jsonl` never do, and with either chosen
  errors are JSON on a terminal too.
- `json` lists stream as one array, one item per line, closed even when a later page fails;
  `yaml` and `table` lists are buffered.
- `table`: an array of objects is one row per object, columns from the keys in order of
  appearance, nested values as compact JSON; an object is key/value lines. On a terminal the
  table fits the width and cells are cut at 48 characters with `…`; off a terminal cells are
  full.
- `--jq <filter>` runs before the format, with jq's standard library (jaq) and `@csv` and
  `@tsv`. On a list it runs on each item, whatever the format and terminal, except with
  `--format json`, where it sees the whole array (`--format json --jq length` counts). String
  results print raw, like `jq -r` and `gh --jq`; others in the format. When the filter fails on
  a response, the response prints unfiltered and the run exits 7 (`output`): a request that
  succeeded is never lost.
- `--max-items N` stops a list after N items; no further page is fetched. A page or cursor
  flag fetches that page only; on a terminal lists stop after the first page, and stderr says
  `--all` fetches the rest.
- Off a terminal output is deterministic: no color, no truncation, keys in the order of the
  SDK's models (`serde_json` with `preserve_order`). Color also goes off with `--no-color`,
  `NO_COLOR` or `TERM=dumb`.
- Writing to a closed pipe (`acme things list | head -1`) ends the run quietly with 0.

### Request bodies

`--data` (`-d`) takes JSON, `@file`, or `-` (also `@-`) for stdin. Field flags then override
properties of it: `kind` coerces the text (`string`, `integer`, `number`, `boolean` with
`--flag` meaning true and `--flag=false`, `json`), a field's `key` is one property even when it
has a dot, `under(&["address"])` puts it in nested objects, created or merged into, `many`
flags repeat to build an array. Errors name the flag:
``--quantity: expected an integer, got `many` ``.

### Schemas and validation

The SDK's models keep what they do not know, unknown properties in their `extra` and unknown
enum and union values as such, so that a newer API does not break them: a CLI sending them
would have the API ignore a misspelled field. So the CLI checks each JSON or form body against
the schema of the spec before sending it, and reports every problem at once, exit 2:

```json
{"error":{"kind":"usage","message":"invalid body: billing_address.postal_code (--billing-address): unknown field; did you mean zip_code?; currency (--currency): required (a string)","errors":[{"path":"billing_address.postal_code","flag":"--billing-address","message":"unknown field; did you mean zip_code?"},{"path":"currency","flag":"--currency","message":"required (a string)"}],...}}
```

It checks unknown fields (suggesting the closest, or listing the valid ones), enum values and
the discriminators of unions (listing the valid ones), missing required fields, JSON types,
integer ranges of their format (`int32`), the formats `date` (`YYYY-MM-DD`), `date-time`,
`uuid`, `decimal`, `email`, `uri`, `hostname`, `ipv4` and `ipv6`, and the constraints of the
spec: bounds, `multipleOf`, lengths, `pattern` (those `regex-bites` can't compile are left to
the API), list sizes and unique items: `quantity (--quantity): must be at most 1000`.
`--allow-unknown-fields` lets unknown fields and values through, for an API newer than the
CLI.

Parameters are checked too, when theirs have constraints: their types by clap and serde,
then their constraints against the schema, all at once before any request, `--dry-run`
included: `invalid parameters: per_page (--per-page): must be at most 100`.

The model's `constraints` (perseid#113) become the JSON Schema keywords of the fields,
parameters and aliases of `schema.json` and of each command, with their `default`,
`examples` and `deprecated`; models without them give schemas without them, and checks of
types and formats only.

`<cli> schema` prints, as JSON, the index of the commands (`command`, `method`, `path`,
`summary`); `<cli> schema customers` those of a resource; `<cli> schema customers create` the
JSON Schema of the command's parameters, keyed by their argument (`--per-page`, `<ID>`), and of
its body, `$ref`s inlined (cycles and deeper levels stay `$ref`s, under `$defs`), with their
constraints, defaults and examples, and an example body; `--response` the schema of what it prints, its `output` (`json`, `jsonl`,
`file`, `text`, `none`) and pagination. `<cli> agents` prints a guide of under 2 KB for scripts
and agents: discovery, input, output, errors and exit codes, `mcp`, retries and `auth token`.

### Dry runs

`--dry-run` prints the request a command would send, as JSON, and exits 0 without sending it:

```json
{"method":"POST","url":"https://api.acme.dev/v1/things","query":{},"headers":{"content-type":"application/json","authorization":"Bearer <redacted>",...},"body":{"name":"Lamp"}}
```

The SDK builds and validates the request as usual: generated code gives its client an
`HttpClient` that records the request and fails the call, with no retries; the run then
prints what was recorded. No credential is needed, the token being a placeholder, and
`authorization`, `proxy-authorization`, `cookie` and `x-api-key` are redacted anyway. Paginated
operations show their first request; files to download need no `--output`. Commands of your
own that do not call the SDK must check `ctx.dry_run()` themselves.

### Errors and exit codes

On a terminal errors print for people (`error: API error 404: No such thing`, the body when it
says more). Otherwise stderr gets one line of JSON, every key always present:

```json
{"error":{"kind":"api","status":429,"message":"API error 429: Slow down","body":{"message":"Slow down"},"request_id":"req_123","retryable":true,"retry_after":30,"errors":null}}
```

`retryable` is true for 408, 429 and 5xx responses and when no response came; `retry_after`
is the response's `Retry-After` in seconds; `errors` lists the problems of an invalid body.

With `--debug` it also has `causes`, the messages of the underlying errors.

| Code | Meaning | `kind` |
|---|---|---|
| 0 | success | |
| 1 | internal error, or a response the SDK could not decode | `internal`, `decode` |
| 2 | usage: arguments, `--data`, field flags, a body the schema rejects, a `--jq` that does not compile, an invalid request | `usage` |
| 3 | the API answered 4xx | `api` |
| 4 | the API answered 5xx (or another non-2xx) | `api` |
| 5 | no response: connection failure, timeout | `network`, `timeout` |
| 6 | configuration: not logged in, login expired or failed, unknown profile, unreadable config | `config` |
| 7 | the request succeeded, but `--jq` failed on the response, which printed unfiltered | `output` |

`--debug` also prints where each credential came from (the base URL without its userinfo), and
the cause chain of errors.

### Retries and timeouts

The SDK retries and times out; the CLI only sets how much. `--retries N` (or
`<PREFIX>_RETRIES`, default the SDK's 2) and `--no-retry` (`--retries 0`) become its
`max_retries`, `--timeout SECS` (or `<PREFIX>_TIMEOUT`, fractions allowed, `0` for none) its
timeout of each attempt. Flags win over the environment; an invalid value is a usage error.

What is retried is the SDK's rule, which the CLI does not loosen: a request that got no
response, or 408, 429 or 5xx, waiting `Retry-After` (or `Retry-After-Ms`) up to a minute, else
backing off from 500ms to 8s. Only when retrying cannot apply it twice: GET, HEAD, PUT, DELETE
and other idempotent methods, and a POST or PATCH carrying an `Idempotency-Key`, which the spec
declares as a header parameter (`--idempotency-key`) or the SDK adds to every POST when
perseid.toml says `idempotency_keys = true`. A POST without one is sent once.

`--debug` logs every attempt on stderr, after it ends, the query and the URL's userinfo left
out: `debug: GET https://api.acme.dev/v1/things?…: 503 in 12ms`, then
`debug: GET https://api.acme.dev/v1/things?…: 200 in 9ms (retry 1)`.

### MCP server

`acme mcp` serves the API to AI agents as a [Model Context Protocol](https://modelcontextprotocol.io)
server on stdin and stdout, newline-delimited JSON-RPC (or over HTTP, see below), written by hand
on `serde_json`. It speaks revision 2026-07-28, whose requests carry their version in
`_meta["io.modelcontextprotocol/protocolVersion"]` (`server/discover`, `tools/list`,
`tools/call`; another version gets error -32022 listing the supported ones), and, for clients
that start with `initialize`, the handshake revisions 2025-11-25, 2025-06-18, 2025-03-26 and
2024-11-05 (`initialize`, `notifications/initialized`, `ping`, `tools/list`, `tools/call`).

A tool per operation of `schema`, but event streams, which never end: named after its command
(`things attachments upload` is `things_attachments_upload`), titled with its summary when short,
described by its summary and route. Its `inputSchema` has a property per parameter, named after
its flag or argument in snake case (`per_page`, `thing_id`), with the parameter's JSON Schema,
`body` for the request body, `max_items` for lists and `jq` for JSON results. File uploads take a
local path. Annotations: `readOnlyHint` for GET and HEAD; `destructiveHint` for DELETE, PUT,
PATCH, and a POST whose verb undoes or ends something (`archive`, `cancel`, `void`, `revoke`...);
`idempotentHint` for GET, HEAD, PUT and DELETE; `openWorldHint` for all.

Schemas are lowered from the OpenAPI flavor `schema` prints to the JSON Schema clients take:
`nullable` becomes `null` in `type`, `oneOf` `anyOf` whose discriminated variants carry their tag
as a `const` (required, so models can fill it), `discriminator`, `x-` keys and `readOnly` fields
(of inputs) go, formats outside JSON Schema's become descriptions (`decimal`: "A decimal number,
as a string"). Claude and VS Code keep `$ref` and `$defs` for cycles; other clients, which
reject them, get every schema inlined, cycles cut. The client is told by its `clientInfo` name,
or `--client claude|vscode|cursor|openai|gemini|other`.

Calls run side by side on one current-thread Tokio runtime, a thread reading stdin, so a slow
list holds back neither the next call nor `ping`; responses come as calls finish, matched by id.
`notifications/cancelled` drops a call, which ends its requests and pages, and it is never
answered. When stdin ends, the calls in flight finish and are answered before the server exits.

A call runs the command it names, in process, with `--format json`: the same clap parsing,
body validation (`--allow-unknown-fields` if the server has it), credentials (the server's
`--profile`, `--api-key`, `--base-url`, environment and config file), retries, `--dry-run` and
`--debug` when the server got them; a body sent as a JSON string is parsed first. Its result is
the JSON response as text and, from revision 2025-06-18, as `structuredContent` too, described
by the tool's `outputSchema`: the response's schema, inlined two levels deep and loosened to what
the SDK decodes (no `required`, closed objects, enums or bounds; `null` allowed below the root).
A response that is not an object is under `result`. A list is `{items, has_more, next, hint}`:
at most `max_items` items (20 by default), the command fetching one more to know whether others
follow, and in `next` the arguments that fetch them, from the pagination of the operation: the
next page number, offset, last item's cursor field, or the `next_cursor` of the page fetched.
`jq` runs on the response (on the items of a list), its output under `jq`, so a model keeps
only the fields it needs. Results longer than `--max-chars` (60,000, under Claude Code's 25,000
tokens) are cut: a list to its first items with a `hint`, a GET to an `output` error naming the
fields to select with `jq`, a write to a note that it succeeded.

A file comes back as an embedded resource, its bytes in base64, up to 8 MiB; larger ones are
refused. A failed command, invalid arguments included, is a result with `isError: true` whose
text is the error JSON of [Errors](#errors-and-exit-codes) without its null keys, for the model
to correct itself; an unknown tool or malformed request is a JSON-RPC error, which suggests the
closest tool or says the server's options left it out.

Toolsets, from `[context.cli_mcp]` in perseid.toml, group tools by name globs, as GitHub's
server has them: `mcp` serves those of `default_toolsets` and the tools no toolset holds, or
those `--toolsets billing,admin` names; `--toolsets all` serves every tool, deprecated ones and
those `hidden` included, which are otherwise left out. A new operation joins its resource's
toolset, so a whole family is hidden in one line. `operations.<tool>` overrides a tool's
`name`, `description`, `title` and `annotations` (merged), or `hidden`; `instructions` comes
first in the server's.

`<tool>` is the name of the command, `name` the one served: toolsets, `--tools`,
`--exclude-tools` and calls take it. It is 1 to 64 of `A-Za-z0-9_.-` (model APIs take 64), and no
other tool's; else `mcp` fails to start, as `mcp::check` in the generated crate's tests does.

`--tools 'customers_*,plans_list'` serves the tools matching a glob, whatever the toolsets;
`--exclude-tools '*_delete'` leaves out those matching one, and `--read-only` serves GET and
HEAD operations only, both whatever `--tools` and the toolsets say. `--list` prints the tools as
the client gets them and exits.

An MCP client launches it with the API key in its environment:

```json
{"mcpServers": {"acme": {"command": "acme", "args": ["mcp"], "env": {"ACME_API_KEY": "sk_..."}}}}
```

or with Claude Code, `claude mcp add acme -e ACME_API_KEY=sk_... -- acme mcp`.

`acme mcp --port 8080` serves the same tools over Streamable HTTP at `http://127.0.0.1:8080/mcp`
(`--host 0.0.0.0` for every interface, `--port 0` for a free port; the URL is printed on stderr),
stateless: each POST is one JSON-RPC message, answered with JSON, never an event stream, by a
server that keeps no session (`Mcp-Session-Id`). Revision 2026-07-28 suits it: its requests carry
their version, `clientInfo` and capabilities in `_meta`, which pick each request's schemas, and
mirror the version, method and tool in `MCP-Protocol-Version`, `Mcp-Method` and `Mcp-Name`
(base64 as `=?base64?…?=`). A header missing, or not the body's value, is a 400 with
`HeaderMismatch` (-32020); an unsupported version a 400 with -32022, a malformed `_meta` a 400
with -32602, an unknown method a 404 with -32601. The earlier revisions are served without a
session: `initialize` is answered and forgotten, so each request names its revision in
`MCP-Protocol-Version` (2025-03-26 without it) and gets the schemas of `--client`. A
notification is answered 202, a GET or DELETE 405, `GET /health` `{"status": "ok", "version"}`.
A client that closes its connection cancels its call.

Each request brings its own API key, as `Authorization: Bearer <KEY>` or in the header of the
spec's `apiKey` security scheme (`X-Api-Key`), and its call runs with it as `--api-key`; a request
without one gets a 401 with `WWW-Authenticate: Bearer realm="mcp"`. The server's credentials
(`--api-key`, `ACME_API_KEY`, the profile) serve no request, unless `--key-from-env`, for a server
only its owner reaches: requests without a key then use them. A request whose `Origin` is neither
localhost's nor one of `--allowed-origins` (`*` for any) gets a 403, against DNS rebinding;
clients other than browsers send none.

```json
{"mcpServers": {"acme": {"type": "http", "url": "http://127.0.0.1:8080/mcp",
                         "headers": {"Authorization": "Bearer sk_..."}}}}
```

or with Claude Code, `claude mcp add --transport http acme http://127.0.0.1:8080/mcp --header
"Authorization: Bearer sk_..."`. HTTP is the runtime's `mcp-http` cargo feature (default): hyper's
server over the HTTP stack the SDK already has, about 320 KB of a release binary. The `auth`
section of `schema.json` names the spec's API key headers.

Clients search large tool lists themselves (Claude Code's tool search), so the server serves
every operation rather than meta-tools that look tools up, which would hide each operation's
schema, annotations and permission from the client. For clients that do not, `--tools` and
`--read-only` keep the list short.

### Credentials and profiles

`<config dir>/<name>/config.toml`, the config dir being `$XDG_CONFIG_HOME` when set, else the
platform's (`~/.config` on Linux, `~/Library/Application Support` on macOS). Written 0600, its
directory 0700, through a temporary file of the process renamed over it; unknown keys survive
rewrites. `config.toml.lock` serializes the runs that rewrite it: refreshes and `auth` commands.

A profile stores a credential, flat in its table: an API key, OAuth tokens, or the fields of a
custom login (unknown keys are its `extra`). With the runtime's `keyring` feature, its secrets
are in the OS keyring instead (see [The OS keyring](#the-os-keyring)).

```toml
default = "prod"

[profiles.prod]
api_key = "sk_live_..."

[profiles.staging]
access_token = "..."
refresh_token = "..."
expires_at = 1760000000   # unix seconds
token_type = "Bearer"
scopes = ["things"]
method = "device"         # the `auth login` flag that made it
base_url = "https://staging.acme.dev"
```

Each value resolves on its own: flag (`--api-key`, `--base-url`, `--profile`) > environment
(`ACME_API_KEY`, `ACME_BASE_URL`, `ACME_PROFILE`) > the selected profile > the SDK's default.
An explicit API key wins over the profile's credential and skips the `Auth` hook; with one, an
unreadable config file or a missing profile is no error (a warning with `--debug`). The profile
is `--profile`, else `ACME_PROFILE`, else `default`, else `"default"`; one named explicitly
must exist.

- `acme auth login [--profile P] [--base-url URL] [--api-key K | --api-key-stdin]` logs in
  and stores the credential. `--api-key` stores that key, `--api-key-stdin` the first line of
  stdin, terminal or not; otherwise the `Auth` hook logs in: by default it prompts for a key
  without echo on a terminal or reads it from stdin (`echo $KEY | acme auth login`). The first
  profile becomes the default.
- `acme auth status` prints the credential commands would use, never its secret:
  `{"profile", "logged_in", "method", "source", "auth", "expires_at", "expires_in",
  "refreshable", "base_url", "storage"}`; `method` is `api_key`, `oauth`, a login flag (`web`,
  `device`) or `custom`, `source` `--api-key`, `$ACME_API_KEY` or the profile, `expires_in` in
  seconds (negative once expired), `storage` `file` or `keyring`. JSON off a terminal; exit 6
  when there is no credential, after printing it.
- `acme auth token` prints the bearer token commands send, refreshed first if it expires within
  a minute: `curl -H "Authorization: Bearer $(acme auth token)" ...`.
- `acme auth logout [--profile P]` lets the hook revoke the credential, removes it (unknown keys
  included), and the profile once it is empty.
- `acme auth profiles` lists them with an `auth` status (`api key sk_live_…1234`,
  `oauth, expires in 12m, refreshable (--web)`), in any `--format`.
- `acme completion bash|zsh|fish|powershell|elvish`; `acme man` prints the man page,
  `acme man --out-dir DIR` writes one per command (hidden from help).

### Authentication

`App::auth` is an `&'static dyn Auth`: `login` for `auth login` (and `login_args`, flags it adds
there), `token` before each command, whose changes to the credential (a refresh) are saved to the
profile, and optional `logout` and `status`. Built in:

- `auth::ApiKey`: today's API key, the default.
- `oauth::DeviceCode` (RFC 8628): prints the verification URL and user code, opens the browser,
  polls the token endpoint honoring `interval` (5 seconds by default, at least 1), `slow_down`
  and `expired_token`. When stderr is not a terminal, an agent's run, it also prints on stdout,
  before polling, `{"verification_uri", "verification_uri_complete", "user_code", "expires_in",
  "next_step"}`, for the agent to hand the user while the command waits.
- `oauth::Browser`: authorization code with PKCE (S256) and `state`, redirected to
  `http://127.0.0.1:<random port>/callback`, a server serving each connection on its own, so an
  idle one (a browser's preconnect) cannot hold up the redirect.
- Both refresh with the `refresh_token` grant a minute before `expires_at`, and revoke on logout
  when given an RFC 7009 `revocation_url`. Their configuration is an OAS `oauth2` flow's.
  Until it expires, a token without a refresh token, or whose token endpoint is unreachable, is
  used as it is.

`ctx.token()` runs the hook under the config file's lock, on the profile as it is once locked:
of concurrent runs needing a refresh, the first refreshes and saves, the others use its token.
- `auth::Methods`: a default and one `auth login --<flag>` per other method.

The browser opens only when stderr is a terminal, with `$BROWSER` if set (`BROWSER=none` never);
the URL is always printed. Behind the runtime's `oauth` cargo feature (default).

```rust
const OAUTH: oauth::Client = oauth::Client::new("acme-cli", "https://auth.acme.dev/token")
    .scopes(&["things"]);
pub const AUTH: Methods = Methods {
    default: &ApiKey,
    methods: &[
        Method::new("web", "Log in with a browser",
            &Browser::new(OAUTH, "https://auth.acme.dev/authorize")),
        Method::new("device", "Log in with a code, from any device",
            &DeviceCode::new(OAUTH, "https://auth.acme.dev/device")),
    ],
};
pub const APP: App = App {
    name: "acme",
    env_prefix: "ACME",
    auth: &AUTH,
    default_base_url: Some("https://api.acme.dev"),
};

// Custom: a browser login whose token mints an API key, the only thing stored.
pub struct WebKey(pub &'static Browser);

impl Auth for WebKey {
    fn login<'a>(&'a self, ctx: &'a mut Ctx, args: &'a ArgMatches)
        -> BoxFuture<'a, Result<Credential, CliError>> {
        Box::pin(async move {
            let session = self.0.login(ctx, args).await?;
            let client = client(ctx, &session.access_token.unwrap_or_default())?;
            Ok(Credential::api_key(client.api_keys().create("acme CLI").await?.secret))
        })
    }

    fn token<'a>(&'a self, ctx: &'a mut Ctx, credential: &'a mut Credential)
        -> BoxFuture<'a, Result<String, CliError>> {
        ApiKey.token(ctx, credential)
    }
}
```

### The OS keyring

With the runtime's `keyring` feature, off in the crate and on in the scaffold's binary,
`auth login` stores `api_key`, `access_token` and `refresh_token` in the OS keyring, an entry
each (service `acme`, user `<profile>:<field>`), and the profile keeps `secrets = "keyring"`
with the rest (`expires_at`, `method`, `scopes`, `base_url`, unknown keys). Refreshed tokens go
back to the keyring, `auth logout` deletes them, and every command reads them, under the
config file's lock like a refresh. The stores are those of
[keyring-core](https://crates.io/crates/keyring-core): the Keychain on macOS, the Credential
Manager on Windows, the Secret Service over D-Bus on Linux and the BSDs, in Rust (zbus): no
`libdbus` or OpenSSL to install.

Without a usable keyring, as on a server or in CI with no Secret Service, `auth login` warns
and stores the secrets in the file, as it does with `--no-keyring` or `ACME_NO_KEYRING=1`. A
profile keeps where its secrets are: files written before the feature keep working, and the
next `auth login` moves them. When the keyring refuses a secret, such as a token over the 2560
bytes of a Windows entry, it goes to the file with a warning, at login as after a refresh.
Secrets of a custom `Auth` other than those three fields stay in the file.

## Development

```sh
cd packs/cli/runtime-crate
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
ACME_API_KEY=sk_test cargo run --example acme -- things list --format table

# The pack, against tests/acme: renders the SDK and the CLI, lints, tests, runs tests/e2e.py
PERSEID=path/to/perseid packs/cli/tests/run.sh
UPDATE_GOLDEN=1 ACME=$PWD/packs/cli/target/debug/acme python3 packs/cli/tests/e2e.py
```

`run.sh` ends with `dist generate --check` and `dist plan` on the rendered repository when
`$DIST` or `dist` is there, at the version `dist-workspace.toml` pins (CI installs it).

`tests/acme/Cargo.lock` and `generated.Cargo.lock` pin the CLI's dependencies, which `run.sh`
builds `--locked`: `UPDATE_LOCKS=1` rewrites them, after a change of dependencies or of perseid.

CI (`.github/workflows/cli.yml`) runs both: the runtime's tests on Linux, macOS and Windows and
its check with Rust 1.89, its `rust-version`; the pack with perseid at a commit of its
`feat/packs` branch until a release has pack targets, then the rendered CLI's tests on macOS
and Windows.

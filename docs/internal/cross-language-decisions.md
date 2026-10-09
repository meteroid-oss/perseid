# Cross-language decisions

Decisions taken while improving one SDK that the other languages must follow. Each open entry
says where it landed and what is left; once every language has it, it moves to the last table.

## Open

| Decision | Done in | To do |
|---|---|---|
| Enum arguments (body fields, query and header parameters) take the enum or its value; in models only requests send, enum fields do too. Models responses carry keep the enum type, so readers' code does not change | Python, TypeScript (closed enums are literal unions), C# (implicit conversions), Rust (`From<String>`, `impl Into`), Java (`String` setters for open enums) | Go: typed constants, a string variable needs `T(s)` |
| Model requests carry also take their JSON as a typed dict (Python's `PetParam`) | Python | Other languages have their own literal or builder form: TypeScript object literals, C# implicit conversions, Rust `From`/`impl Into`, Java builder setters per variant, Go structs |

## Done in every language

| Decision |
|---|
| `[models]` in `perseid.toml` renames types (`CreateChatCompletionResponse = "ChatCompletion"`), before the spec model names anything: nested types follow, implicit discriminator tags keep the schema name; `init --from stainless` imports Stainless' `models` (spec model) |
| The boolean `stream` part of a multipart body with an event-stream twin is no field of the body: the twin sends `true`, the other method nothing (`stream_part`, spec model) |
| Optional nullable fields tell `null` from absent (Rust `Option<Option<T>>` + `clear_*`, Python `UNSET`) only in request-only models and PATCH bodies; models responses also carry read `Option<T>` / `X \| None`. Python method arguments still send `None` as `null` (`with_nulls`). Go, Java, C# and TypeScript have their own null forms |
| `allOf` parts are no parent model to build: Rust, Go, C# inline them (strict), Python (lenient); Java embeds them with `@JsonUnwrapped`; TypeScript `extends` them, which type-checks flat literals and hovers the same |
| Binary responses are returned once their headers arrive, read lazily: whole, as chunks, or to a file (Go, Java and C# write a temp file renamed over the target). Errors raise before; retries stop at the headers; the timeout covers the headers then each read, not the download. Python `BinaryResponse`/`AsyncBinaryResponse`, TypeScript `BinaryResponse` (`asResponse()`), Rust `BinaryResponse` (`Stream`, `AsyncRead`), Go `*BinaryResponse` (`io.ReadCloser`), Java and C# `BinaryResponse` |
| A 429 is retried whatever the method, when the body can be sent again: the server refused the request |
| Lists of OpenAI's and Anthropic's shape (`after`/`after_id` taking `last_id`, while `has_more`) page without a rule (spec model) |
| Open enums (`anyOf: [string, enum]`) carry `open: true` (from `x-perseid-open-enum`, set by the spec normalization) and accept any string next to their values |
| Multipart file fields also take a path, read and named after it (Python `Path`, Rust `Upload::path`, Go `UploadFile`, Java `Upload.of(Path)`, TypeScript `{ path }` on Node, C# `Upload.FromFile`). The part's type is the upload's own, else the spec's unless `application/octet-stream`, else the file extension |
| Union variants sharing a tag (OpenAI's `InputItem`: three `message` variants) are sent with the tag they declare, not their schema name; decoding finds the variant the data fits (the next one that decodes, or the one knowing most of its properties where decoding is lenient). In the spec model, a variant's implicit (schema-name) tag no longer erases the constant its property declares |
| Merged `allOf` parts typed the same once `$ref`s are followed and `X \| null` is unwrapped, instead of an untyped value (spec normalization) |
| An error event in a stream raises an API error whatever its data (JSON or not); so does data whose object has a non-null `error` the event model fails to decode or does not declare (Go's rule). `ping` and `keepalive` events are skipped when their data is not the model |
| Errors show the API's message (`error.message`, `message`, `detail`), the raw body kept |
| `init` sets `timeout = 600` for specs with event streams (LLM APIs answer slowly), as Stainless' 10 minutes |
| A binary or text response that may also be an event stream (OpenAI's speech) returns the content, with a `_stream` twin, instead of only a stream (spec model) |
| `perseid init` keeps a mixed-case title word as written (`OpenAI`, `GitHub`) and one word in packages, Go module, header prefix, user agent and `env_prefix` (`openai`, `OPENAI_API_KEY`); the Java package skips `help`, `support` and `platform` subdomains |

## Considered and left as is

- Type names keep acronyms camel-cased (`OpenAiFile` for `OpenAIFile`), in every language. PEP 8
  prefers `OpenAIFile`, but keeping one rule avoids names that differ between SDKs and schemas
  whose names only differ in case colliding.
- A hand-written `name = "OpenAI"` still gives `open_ai` packages: `OpenAI` and `PetStore` cannot
  be told apart. `[python] package` and `[context] env_prefix` set them; `init` does it for you.
- POSTs are not retried on 5xx or connection errors without an idempotency key, deliberately:
  replaying them could apply them twice. Stainless and Fern retry them. `idempotency_keys = true`
  sends a key with every POST and retries them.
- Required multipart bodies are keyword arguments in Python only. Other languages keep a body
  type: their multipart bodies are rare and the type carries files and parts per variant.
- Java's nested variant classes keep their schema names, so `InputItem.InputMessage` hides the
  `InputMessage` model inside the union's file: renaming would rename most variants.
- Rust's shorter `ArrayOf...` variant names would rename public variants users match on.

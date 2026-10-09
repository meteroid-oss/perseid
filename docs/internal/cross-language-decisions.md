# Cross-language decisions

Decisions taken while improving one SDK that the other languages must follow. Each open entry
says where it landed and what is left; once every language has it, it moves to the last table.

## Open

| Decision | Done in | To do |
|---|---|---|
| `allOf` parts are inlined into the model's own fields, so callers never build the parent model. A part that is not an object keeps today's embedding instead of failing generation | Rust, Go, C# (strict), Python (lenient), Java | Check TypeScript |
| Required multipart bodies are keyword arguments / flat options, like JSON bodies; no `...Body` class is generated for them | Python | Every other language |
| The boolean `stream` part of a multipart `_stream` twin is set by the twins (`true` on the stream one, left out of the other), like JSON bodies' | Python, Go, Java, C# | Rust, TypeScript; consider moving it to `stream_property` in `src/api/resources.rs` |
| Enum arguments (body fields, query and header parameters) take the enum or its value; in models only requests send, enum fields do too. Models responses carry keep the enum type, so readers' code does not change | Python, TypeScript (closed enums are literal unions), C# (implicit conversions), Rust (`From<String>`, `impl Into`), Java (`String` setters for open enums) | Go: typed constants, a string variable needs `T(s)` |
| Responses can be streamed without loading them (Stainless' `with_streaming_response`) | — | Every language (C# would add interface methods, breaking users' fakes) |
| Types can be renamed in `perseid.toml` (Stainless turns `CreateChatCompletionResponse` into `ChatCompletion`) | — | Every language |
| Model requests carry also take their JSON as a typed dict (Python's `PetParam`) | Python | Other languages have their own literal or builder form: TypeScript object literals, C# implicit conversions, Rust `From`/`impl Into`, Java builder setters per variant, Go structs |

## Done in every language

| Decision |
|---|
| A 429 is retried whatever the method, when the body can be sent again: the server refused the request |
| Lists of OpenAI's and Anthropic's shape (`after`/`after_id` taking `last_id`, while `has_more`) page without a rule (spec model) |
| Open enums (`anyOf: [string, enum]`) carry `open: true` (from `x-perseid-open-enum`, set by the spec normalization) and accept any string next to their values |
| Multipart file fields also take a path, read and named after it (Python `Path`, Rust `Upload::path`, Go `UploadFile`, Java `Upload.of(Path)`, TypeScript `{ path }` on Node, C# `Upload.FromFile`) |
| Union variants sharing a tag (OpenAI's `InputItem`: three `message` variants) are sent with the tag they declare, not their schema name; decoding finds the variant the data fits (the next one that decodes, or the one knowing most of its properties where decoding is lenient). In the spec model, a variant's implicit (schema-name) tag no longer erases the constant its property declares |
| Merged `allOf` parts typed the same once `$ref`s are followed and `X \| null` is unwrapped, instead of an untyped value (spec normalization) |
| An error event in a stream (`event: error`, or data with an `error` key the event model cannot take) raises an API error, not a decode error; keepalive events are skipped |
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
- POSTs are not retried on 5xx or connection errors without an idempotency key: replaying them
  could apply them twice. Stainless and Fern retry them. `idempotency_keys = true` sends a key
  with every POST and retries them; whether `init` should turn it on is an open question.
- Java's nested variant classes keep their schema names, so `InputItem.InputMessage` hides the
  `InputMessage` model inside the union's file: renaming would rename most variants.
- Rust's shorter `ArrayOf...` variant names would rename public variants users match on.

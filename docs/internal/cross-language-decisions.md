# Cross-language decisions

Decisions taken while improving one SDK that the other languages must follow. Each entry says
where it landed and what is left. Delete an entry once every language has it.

| Decision | Done in | To do |
|---|---|---|
| `allOf` parts are inlined into the model's own fields, so callers never build the parent model. A part that is not an object keeps today's embedding instead of failing generation | Rust, Go, C# (strict), Python (lenient) | TypeScript, Java |
| Object request bodies are always keyword arguments / flat options, `allOf` included | Python | Check TypeScript, Java, C#, Go, Rust |
| Required multipart bodies are keyword arguments, like JSON bodies; no `...Body` class is generated for them | Python | Every other language |
| The boolean `stream` part of a multipart `_stream` twin is set by the twins (`true` on the stream one, left out of the other), like JSON bodies' | Python | Every other language; consider moving it to `stream_property` in `src/api/resources.rs` |
| Open enums (`anyOf: [string, enum]`) carry `open: true` (from `x-perseid-open-enum`, set by the spec normalization) and accept any string next to their values | Python | Every other language: their enums keep unknown values, but their argument types may not accept a plain string |
| Enum arguments (body fields, query and header parameters) take the enum or its value; in models only requests send, enum fields do too. Models responses carry keep the enum type, so readers' code does not change | Python | Check TypeScript, Java, C#, Go |
| Models requests carry also take their JSON as a typed dict (`PetParam`, keys as on the wire, the tag of a union variant required); `param_types` in templates lists them. Model types themselves do not change | Python | TypeScript already takes object literals; check Go, Java, C#, Rust have a builder or literal form |
| A 429 is retried whatever the method, when the body can be sent again: the server refused the request | Every language | — |
| Lists of OpenAI's and Anthropic's shape (`after`/`after_id` taking `last_id`, while `has_more`) page without a rule | Every language (spec model) | — |
| Multipart file fields also take a path, read and named after it | Python | Rust (`PathBuf`), TypeScript (Node `fs` path?), Go, Java (`Path`), C# (`FileInfo`) |
| Responses can be streamed without loading them (Stainless' `with_streaming_response`) | — | Every language |
| Types can be renamed in `perseid.toml` (Stainless turns `CreateChatCompletionResponse` into `ChatCompletion`) | — | Every language |
| Union variants sharing a tag (OpenAI's `InputItem`: three `message` variants) are sent with the tag they declare, not their schema name; decoding tries the other variants declaring the tag when the first rejects the data. In the spec model, a variant's implicit (schema-name) tag no longer erases the constant its property declares | Python, spec model | TypeScript (sends `type: "InputMessage"` today), Rust, Go, Java, C# |
| Merged `allOf` parts typed the same once `$ref`s are followed and `X \| null` is unwrapped, instead of an untyped value (`CreateResponse.top_logprobs`...) | Every language (spec normalization) | — |
| An error event in a stream (`event: error`, or data with an `error` key) raises an API error, not a decode error | — | Check every language |
| `init` sets `timeout = 600` for specs with event streams (LLM APIs answer slowly), as Stainless' 10 minutes | Every language (`init`) | — |
| `perseid init` keeps a mixed-case title word as written (`OpenAI`, `GitHub`) and one word in packages, header prefix, user agent and `env_prefix` (`openai`, `OPENAI_API_KEY`) | Every language (init) | — |

## Considered and left as is

- Type names keep acronyms camel-cased (`OpenAiFile` for `OpenAIFile`), in every language. PEP 8
  prefers `OpenAIFile`, but keeping one rule avoids names that differ between SDKs and schemas
  whose names only differ in case colliding.
- A hand-written `name = "OpenAI"` still gives `open_ai` packages: `OpenAI` and `PetStore` cannot
  be told apart. `[python] package` and `[context] env_prefix` set them; `init` does it for you.
- POSTs are not retried on 5xx or connection errors without an idempotency key: replaying them
  could apply them twice. Stainless and Fern retry them. `idempotency_keys = true` sends a key
  with every POST and retries them; whether `init` should turn it on is an open question.

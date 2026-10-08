# Cross-language decisions

Decisions taken while improving one SDK that the other languages must follow. Each entry says
where it landed and what is left. Delete an entry once every language has it.

| Decision | Done in | To do |
|---|---|---|
| `allOf` parts are inlined into the model's own fields, so callers never build the parent model. A part that is not an object keeps today's embedding instead of failing generation | Rust, Go, C# (strict), Python (lenient) | TypeScript, Java |
| Object request bodies are always keyword arguments / flat options, `allOf` included | Python | Check TypeScript, Java, C#, Go, Rust |
| Required multipart bodies are keyword arguments, like JSON bodies; no `...Body` class is generated for them | Python | Every other language |
| The boolean `stream` part of a multipart `_stream` twin is set by the twins (`true` on the stream one, left out of the other), like JSON bodies' | Python | Every other language; consider moving it to `stream_property` in `src/api/resources.rs` |

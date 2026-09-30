# Customizing

Four extension points, none of which needs a fork.

1. **Handwritten files.** Perseid never deletes a file without the `@generated` marker, and refuses to
   overwrite one: put your modules next to the generated ones. In Go, define extra methods on the
   generated resource types from any file of the package.
2. **Middleware.** Every SDK lets you wrap each HTTP attempt to cache, log, sign or rewrite requests.
3. **Resource snippets.** A file at `.perseid/templates/<lang>/extensions/<resource>.<ext>` is inlined
   in the body of that resource's class (Rust, TypeScript, Java, C#, and Python where
   `<resource>_async.py` targets the async class), to add custom methods.
4. **Ejected templates and runtime.** `perseid eject <lang>` copies the built-in Jinja templates and
   runtime to `.perseid/`. Files you keep there override the built-ins; delete the rest to keep
   receiving upstream updates. `context` tables in `perseid.toml` reach templates as `sdk.*`.

## Middleware

| Language | Option |
|---|---|
| TypeScript | `middleware`: `(request, next) => Response` |
| Python | `middleware` and `async_middleware` |
| Go | `Options.Middleware`, a `RoundTripper` wrapper |
| Java | `getInterceptors()`, OkHttp interceptors, also added to a client given to `setHttpClient` |
| Rust | `Middleware` trait, `options.middleware` |
| C# | `Handlers`, `DelegatingHandler`s, also run in front of your own `HttpClient` |

Middleware runs inside the retry loop, after credentials are set, and the first one registered is
the outermost.

A local cache for GET endpoints, in TypeScript:

```ts
const cache = new Map<string, string>();
const petstore = new Petstore("sk_live_...", {
  middleware: [
    async (request, next) => {
      if (request.method !== "GET" || !new URL(request.url).pathname.startsWith("/pets")) {
        return next(request);
      }
      const hit = cache.get(request.url);
      if (hit) return new Response(hit, { headers: { "content-type": "application/json" } });
      const response = await next(request);
      if (response.ok) cache.set(request.url, await response.clone().text());
      return response;
    },
  ],
});
```

and in Rust:

```rust
struct Cache(Mutex<HashMap<String, Bytes>>);

impl Middleware for Cache {
    fn handle<'a>(&'a self, request: Request, next: Next<'a>) -> BoxFuture<'a, Result<Response, BoxError>> {
        Box::pin(async move {
            let key = request.uri().to_string();
            if request.method() != Method::GET || !request.uri().path().starts_with("/pets") {
                return next.run(request).await;
            }
            if let Some(body) = self.0.lock().unwrap().get(&key).cloned() {
                return Ok(Response::buffered(StatusCode::OK, HeaderMap::new(), body));
            }
            let response = next.run(request).await?;
            if !response.status().is_success() {
                return Ok(response);
            }
            let (status, headers, body) = response.into_parts().await?;
            self.0.lock().unwrap().insert(key, body.clone());
            Ok(Response::buffered(status, headers, body))
        })
    }
}

let mut options = PetstoreOptions::default();
options.middleware.push(Cache(Default::default()));
```

## Webhooks

Set `webhooks = true` (top-level or per language table) to install a dependency-free
[Standard Webhooks](https://www.standardwebhooks.com) verifier, also accepting Svix's `svix-*` headers:

```ts
new Webhook("whsec_...").verify(rawBody, request.headers); // throws WebhookVerificationError
```

It is `Webhook` in every language: `Webhook::new(secret)?.verify(&body, &headers)` in Rust,
`NewWebhook(secret)` then `Verify(body, r.Header)` in Go, `new Webhook(secret).Verify(body, name =>
Request.Headers[name])` in C#. Payload models come from `webhooks` and `x-webhooks` in the spec.
In Rust the verifier lives behind the `webhooks` cargo feature that `init` adds to `Cargo.toml`
and `lib.rs`.

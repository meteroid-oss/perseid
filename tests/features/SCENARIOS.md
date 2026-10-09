# Smoke test scenarios

Shared contract between `mock_server.py`, `tests/fixtures/features.yaml` and the six per-language
smoke tests. Every smoke test implements every scenario below identically, with the same inputs
and the same assertions. Operation ids are the spec ids; each SDK spells them in its own
convention (for example `get_item`, `getItem`, `GetItem`).

## Server rules

* The server checks method, RAW path, RAW query, required headers, content type and body against
  the spec. A mismatch answers `400` with JSON
  `{"error", "operation", "problems": [...], "received": {...}}`; a wrong method answers `405`
  with an `Allow` header; an unknown path answers `404`. A smoke test that sends a sloppy request
  therefore fails with the exact reason in the SDK's error body.
* Percent-encoding is checked on the raw request line: every byte outside
  `A-Za-z0-9-._~` and the RFC 3986 sub-delims (`!$&'()*+,;=`, plus `:@/` in paths and `:@/?[]`
  in queries) must be a `%XX` escape with UPPERCASE hex digits. UTF-8 is encoded byte by byte.
  A space may be `%20` (or `+` in a query); a literal `+` in a query value must be `%2B`.
* Operations with `security: []` must carry no credentials (`Authorization`, `X-API-Key`,
  `api_key` query, `auth_token` cookie), even when the client was built with a token.
* Operations with the default security need a bearer token or an API key; missing credentials
  answer `401 {"error":"unauthorized"}`.
* `X-Scenario-Id` scopes per-test server state. Use a unique id per test case (for example
  `<lang>-<scenario>-<n>`). `GET /__server/attempts/<id>` (not part of the spec; plain HTTP, no SDK)
  returns `{"id", "attempts", "keys"}` so a test can assert how many attempts the server saw and
  which idempotency keys.
* The base URL is overridden to the mock server; the spec's `servers` entry only needs to carry
  the variables `{region}` (default `eu`, enum `eu|us`) and `{version}` (default `v1`).
* "Error" below means the SDK's typed API error: it must expose the HTTP status and the raw or
  parsed body (`{"error": ..., "code": ...}`).
* Retry configuration: scenarios that need retries enable at least 1 retry (use 2). Scenarios
  that must not retry disable retries explicitly. Waiting is allowed to take up to about 3 seconds.

## Pre-existing operations

`list_widgets`, `list_widget_events`, `list_gadgets`, `list_records` (pagination), `health`,
`create_session`, `machine_status` (auth variants), `stream_events` (SSE), `upload_file`,
`upload_content`, `search`, `create_charge`, `beta_search`, `put_image` keep their previous
contract: the JSON body `{"status": ...}` echoes what the server read, with the formats the
existing smoke tests already assert (`auth()` = `Authorization|X-API-Key|api_key`, sorted decoded
query pairs, multipart summary, and so on). `stream_events` is now delivered with chunked
transfer-encoding, one chunk per event with a short pause between chunks, same events as before.
`create_completion` answers `{"text": PROMPT}` (the prompt upper-cased), or with `stream: true`
one `data: {"delta": c, "index": i}` event per character of the prompt, then `data: [DONE]`.
`list_widgets` items carry a `color` the spec does not declare, which SDKs keep as an unknown
property. Every response carries `x-request-id: req_mock`.

## Items: methods and empty responses

| Id | Send | Server returns | Client asserts |
|---|---|---|---|
| `get_item` | `GET /items/i1`, item_id `i1` | `200 {"id":"i1","name":"first","note":"hi"}` | id `i1`, name `first`, note `hi` |
| `patch_item` | `PATCH /items/i1`, `Content-Type: application/json`, body exactly `{"name":"renamed"}` (`note` UNSET and therefore omitted, not `null`) | `200 {"id":"i1","name":"renamed","note":null}` | name `renamed`, note is null/None/absent-as-null |
| `delete_item` | `DELETE /items/i1`, no body, no content type needed | `204`, no body, no content-length | call succeeds and returns nothing/unit/void |

## Content and decoding

| Id | Send | Server returns | Client asserts |
|---|---|---|---|
| `nullable_body` | `GET /scenarios/nullable-body` | `200 application/json` body `null` | result is null/None/Option::None/nullable-empty, no decode error |
| `text_plain` | `GET /scenarios/text` | `200 text/plain; charset=utf-8` body `hello text\n` | string equals `"hello text\n"` |
| `text_csv` | `GET /scenarios/csv` | `200 text/csv; charset=utf-8` body `id,name\n1,alpha\n2,"be,ta"\n` | string equals that text exactly |
| `download_blob` | `GET /scenarios/blob` | `200 application/octet-stream`, the 256 bytes `0x00..0xFF` in order, chunked in 8 parts of 32 bytes 50 ms apart | bytes equal `bytes(range(256))`, length 256 (not decoded as text); every SDK streams them: read whole and chunk by chunk, with status 200 and that content type; Go, Java and C# also write them to a file, with a 250 ms timeout the whole download outlasts (it bounds each read, not the body) |
| `download_image` | `GET /scenarios/image` | `200 image/png`, bytes `89 50 4E 47 0D 0A 1A 0A` followed by `00 01 FE FF` repeated 4 times (24 bytes) | bytes equal those 24 bytes |
| `malformed_json` | `GET /scenarios/malformed` | `200 application/json` body `{"status": ` (truncated) | a decode/deserialization error is raised; never a silent default |
| `empty_body` | `GET /scenarios/empty-body` | `200 application/json`, empty body | a decode error is raised (the response type is a required object) |
| `extra_fields` | `GET /scenarios/extra-fields` | `200 {"status":"ok","extra":1,"nested":{"a":[1,2,{"b":null}]},"list":[1,"x"]}` | succeeds; `status == "ok"`; unknown fields ignored |
| `get_nulls` | `GET /scenarios/nulls` | `200 {"name":null,"tags":["a",null,"b"],"counts":{"x":1,"y":null},"note":null}` | name null; tags `["a",null,"b"]`; counts x=1, y=null; note null |
| `echo_nulls` | `POST /scenarios/nulls`, `application/json`, body exactly `{"name":null,"tags":["a",null],"counts":{"x":null,"y":2}}`: `name` is an explicit null (sent as `null`), `note` is unset (omitted) | `200` with the same JSON | response equals what was sent, nulls preserved |
| `get_bag` | `GET /scenarios/bag` | `200 {"id":"b1","a":1,"b":2}` | id `b1`; additional properties `a=1`, `b=2` readable |
| `get_labels` | `GET /scenarios/labels` | `200 {"k":"v","z":"y"}` | map equals `{k: v, z: y}` |
| `enum_value` (known) | `GET /scenarios/enum?mode=known` | `200 {"kind":"red","kinds":["green","blue"]}` | kind red; kinds green, blue |
| `enum_value` (unknown) | `GET /scenarios/enum?mode=unknown` | `200 {"kind":"magenta","kinds":["red","magenta"]}` | no error; the unknown value `magenta` is preserved (raw string or an Unknown variant carrying it), `kinds[0]` is red |

## Encoding

| Id | Send | Server returns | Client asserts |
|---|---|---|---|
| `big_int` | `POST /scenarios/bigint`, body with integer literals `value = 9007199254740993` (2^53 + 1) and `min = -9223372036854775808`; no float notation | `200 {"value":9007199254740993,"min":-9223372036854775808}` | both values round-trip exactly (use 64-bit integer types, `BigInt` in TypeScript) |
| `bytes_echo` | `POST /scenarios/bytes`, field `data` = bytes `68 65 6C 6C 6F FB FF FE` (`hello` + 3 high bytes), sent as standard-alphabet padded base64 `aGVsbG/7//4=` | `200 {"data":"aGVsbG/7//4="}` | decoded bytes equal the sent bytes |
| `bytes_get` | `GET /scenarios/bytes` | `200 {"data":"aGVsbG/7//4="}` | decoded bytes equal `hello\xfb\xff\xfe` |
| `datetime_query` | `GET /scenarios/datetime?since=<T>&day=2024-01-02` where T is the instant `2024-01-02T03:04:05.250Z` (any RFC 3339 spelling of that instant; a `+` offset must be sent as `%2B`) | `200 {"at":"2024-01-02T03:04:05.250Z","day":"2024-01-02"}` | parsed `at` equals the instant 2024-01-02 03:04:05.250 UTC; `day` is 2024-01-02 |
| `datetime_body` | `POST /scenarios/datetime`, body object with exactly `at` (the same instant) and `day` (`2024-01-02`) | same response as above | parsed values equal the same instant and date |
| `path_segment` | one call per value `V` in: `plain`, `sp ace`, `sl/ash`, `q?mark`, `per%cent`, `ha#sh`, `lit%25eral`, `a+b`, `héllo wörld ✓`; path `/scenarios/paths/<V encoded>`. Encoded forms: space `%20`, `/` `%2F`, `?` `%3F`, `%` `%25`, `#` `%23`, `%25` as `%2525`, UTF-8 bytes as uppercase `%XX` (`+` may stay or be `%2B`) | `200 {"status": <V decoded by the server>}` | `status == V` for every value |
| `query_text` | one call per value `V` in: `plain`, `sp ace`, `a&b=c+d`, `100%`, `slash/qm?`, `héllo wörld ✓`; query `q=<V encoded>` (`&` `%26`, `=` `%3D` or raw, `+` `%2B`, `%` `%25`, space `%20` or `+`) | `200 {"status": V}` | `status == V` for every value |
| `query_multi` | `GET /scenarios/multi` with `ids=["b","a","c"]` exploded in this order and `flag=true`: raw query contains `ids=b`, `ids=a`, `ids=c` in that order | `200 {"status":"b,a,c"}` | status `b,a,c` |
| `tenant_headers` | header `X-Tenant: acme`, second call also `X-Trace-Id: t1`; the first call must not send `X-Trace-Id` | `200 {"status":"acme|"}` then `{"status":"acme|t1"}` | statuses `acme|` and `acme|t1` |

## Cookies

| Id | Send | Server returns | Client asserts |
|---|---|---|---|
| `cookie_session` | cookie parameter `session_id = "abc123"`: header `Cookie: session_id=abc123` | `200 {"status":"abc123"}` | status `abc123` |
| `cookie_auth` | client built with the cookie key `ck1` for security scheme `api_key_cookie` (apiKey in cookie `auth_token`): header `Cookie: auth_token=ck1`, no `Authorization` | `200 {"status":"ck1"}`; without the cookie `401` | status `ck1` |

## OAuth2 client credentials

`machine_status` takes the `oauth` scheme (`clientCredentials`, `tokenUrl: /oauth/token` relative to
the base URL, scope `machines.read`). The SDK is built with a client id and the client secret
`p@ss word` (no token, no token provider) and fetches the token itself:
`POST /oauth/token` with `grant_type=client_credentials&scope=machines.read`, the credentials in an
HTTP basic `Authorization` header form-encoded before the base64 (RFC 6749 2.3.1), or, with the
client auth option set to `body`, as `client_id` and `client_secret` form fields. The token endpoint
answers `200 {"access_token":"at-<client id>-<n>","token_type":"Bearer","expires_in":3600}`, `n`
counting the tokens issued to that client id, and `401 {"error":"invalid_client"}` for a wrong
secret. `GET /__server/attempts/<client id>` counts the token requests. `<lang>` is the language
of the smoke test.

| Id | Send | Server returns | Client asserts |
|---|---|---|---|
| `oauth_fetch` | client id `<lang>-oauth`, `machine_status` called twice | `200 {"status":"Bearer at-<lang>-oauth-1||"}` both times | both statuses `Bearer at-<lang>-oauth-1||`; the attempts of `<lang>-oauth` is `1` (one token request, token cached) |
| `oauth_body` | client id `<lang>-oauth-body`, client auth in the body, `machine_status` once | `200 {"status":"Bearer at-<lang>-oauth-body-1||"}` | status `Bearer at-<lang>-oauth-body-1||` |
| `oauth_renew` | client id `<lang>-oauth-revoked`, `machine_status` once: the API revokes the first token issued to a client named `*-revoked-*` | first call with token `...-1`: `401`; the client fetches another token and sends the call again: `200 {"status":"Bearer at-<lang>-oauth-revoked-2||"}` | status `Bearer at-<lang>-oauth-revoked-2||`; the attempts of `<lang>-oauth-revoked` is `2`; retries may be disabled |
| `oauth_invalid` | client id `<lang>-oauth-bad` with another secret, `machine_status` once, retries disabled | token endpoint `401 {"error":"invalid_client"}` | the call fails with the SDK's `401` error, the body `{"error":"invalid_client"}` |

## Retries and idempotency

Retries enabled (2) unless stated. The first retry delay comes from `Retry-After`.

| Id | Send | Server returns | Client asserts |
|---|---|---|---|
| `flaky_service` | header `X-Scenario-Id: <unique>` | attempt 1: `503 {"error":"unavailable"}` with `Retry-After: 1`; attempt 2: `200 {"status":"attempt=2"}` | result `attempt=2`; server `attempts == 2`; elapsed time at least about 1 second (when the language can measure it) |
| `flaky_service` (no retries) | same, retries disabled, fresh id | `503` | Error with status 503; server `attempts == 1` |
| `rate_limited` | header `X-Scenario-Id` | attempt 1: `429 {"error":"slow down"}` with `Retry-After` as an HTTP-date (now + 2 s, `Wdy, DD Mon YYYY HH:MM:SS GMT`); attempt 2: `200 {"status":"attempt=2"}` | result `attempt=2`; attempts 2; the date form is parsed (no crash, no immediate retry storm) |
| `always_unavailable` | header `X-Scenario-Id`, retries = 2 | every attempt `503` with `Retry-After: 0` | Error with status 503 after exhausting retries; server `attempts == 3` |
| `idempotent_create` | `POST /scenarios/idempotent`, headers `X-Scenario-Id: <unique>` and `Idempotency-Key: idem-1`, body exactly `{"amount":5}` | attempt 1: `503` with `Retry-After: 0`; attempt 2: `200 {"status":"attempts=2;key=idem-1"}`; a different key on the retry answers `400` | result `attempts=2;key=idem-1`; server `keys == ["idem-1","idem-1"]` |

## Errors and pagination

| Id | Send | Server returns | Client asserts |
|---|---|---|---|
| `status_error` | one call per code in `400, 401, 403, 404, 409, 422` | that status with `{"error":"status <code>","code":<code>}` | Error with status equal to the code; body exposes `error` and `code`; no retry (server answers once) |
| `list_pages` | iterate all items with the auto-paginating iterator (no cursor first) | page 1: `{"data":[{"id":"p1","name":"p1"},{"id":"p2","name":"p2"}],"next_cursor":"c2"}`; `cursor=c2`: `409 {"error":"page_gone","code":409}` | iteration yields `p1`, `p2`, then raises Error with status 409 (the error is not swallowed and iteration does not loop) |

## Streaming

| Id | Send | Server returns | Client asserts |
|---|---|---|---|
| `sse_chunked` | `GET /scenarios/sse` | `200 text/event-stream; charset=utf-8`, chunked transfer-encoding, 9 chunks about 30 ms apart, split mid-field: `": ping\n\n"`, `"data: first\n\n"`, `"event: tick\nid: 7\nda"`, `"ta: line1\nda"`, `"ta: line2\n\n"`, `"retry: 2500\r\n"`, `"data: {\"n\": 3}\r\n\r"`, `"\nda"`, `"ta: tail\n\n"` | exactly 4 events, in order: (`message`, `first`), (`tick`, `line1\nline2`), (`message`, `{"n": 3}`), (`message`, `tail`). Where the SDK exposes them: ids are none, `7`, `7`, `7` and retry is unset, unset, `2500`, unset (the id persists across events as in the SSE spec, retry applies only to the event it arrives on). The comment line yields no event. |
| `sse_denied` | `GET /scenarios/sse-error` | `403 application/json {"error":"forbidden","code":403}` | Error with status 403 raised before any event is yielded |

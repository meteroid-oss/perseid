"""Strict conformance mock of tests/fixtures/features.yaml for the generated SDK smoke tests.

Prints its base URL on the first line of stdout, then serves until killed.

The route table below mirrors the spec by hand (stdlib only, no YAML parser). Every request is
checked against it: method (405), raw path and raw query string (exact percent-encoding),
required headers, content type and body. Any mismatch answers 400 with a JSON body listing every
problem, so a smoke test fails loudly instead of silently passing on a sloppy request.
The per-scenario contract lives in tests/features/SCENARIOS.md.
"""

import base64
import json
import re
import string
import sys
import threading
import time
import traceback
from datetime import datetime, timedelta, timezone
from email.message import Message
from email.utils import formatdate
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qsl, unquote, unquote_plus

WIDGETS = {None: (["w1", "w2"], "c2"), "c2": (["w3"], None)}
EVENTS = {None: (["e1", "e2"], True), "e2": (["e3"], False)}
# By `ending_before`: the events before it, newest first, as Stripe pages backwards.
EVENTS_BEFORE = {"e9": (["e7", "e8"], True), "e7": (["e6"], False)}
GADGETS = {0: ["g1", "g2"], 1: ["g3"]}
RECORDS = {0: ["r1", "r2"], 2: ["r3"]}
STREAM = [
    ": keep-alive\n\n",
    "event: greeting\nid: 1\ndata: TOPIC\n\n",
    "data: line1\ndata: line2\n\n",
    'id: 3\r\nretry: 1500\r\ndata: {"n": 3}\r\n\r\n',
]
# Chunk boundaries fall in the middle of field names, values and CRLF pairs on purpose.
SSE_CHUNKS = [
    ": ping\n\n",
    "data: first\n\n",
    "event: tick\nid: 7\nda",
    "ta: line1\nda",
    "ta: line2\n\n",
    "retry: 2500\r\n",
    'data: {"n": 3}\r\n\r',
    "\nda",
    "ta: tail\n\n",
]

BLOB = bytes(range(256))
IMAGE = b"\x89PNG\r\n\x1a\n" + b"\x00\x01\xfe\xff" * 4
BYTES = b"hello\xfb\xff\xfe"
BYTES_B64 = base64.b64encode(BYTES).decode()  # aGVsbG/7//4=
PATH_CASES = {
    "plain",
    "sp ace",
    "sl/ash",
    "q?mark",
    "per%cent",
    "ha#sh",
    "lit%25eral",
    "a+b",
    "h\u00e9llo w\u00f6rld \u2713",
}
QUERY_CASES = {
    "plain",
    "sp ace",
    "a&b=c+d",
    "100%",
    "slash/qm?",
    "h\u00e9llo w\u00f6rld \u2713",
}
STATUS_CODES = {400, 401, 403, 404, 409, 422}
INSTANT = datetime(2024, 1, 2, 3, 4, 5, 250000, tzinfo=timezone.utc)

UNRESERVED = set(string.ascii_letters + string.digits + "-._~")
SUBDELIMS = set("!$&'()*+,;=")
PATH_OK = UNRESERVED | SUBDELIMS | set(":@/")
QUERY_OK = UNRESERVED | SUBDELIMS | set(":@/?[]")
HEX = set("0123456789ABCDEF")

LOCK = threading.Lock()
ATTEMPTS = {}  # scenario id -> {"attempts": int, "keys": [idempotency keys]}
MISSING = object()


class Reply:
    def __init__(self, status=200, body=b"", ctype="application/json", headers=None):
        self.status, self.body, self.ctype, self.headers = status, body, ctype, headers or {}


class Chunked:
    def __init__(self, chunks, ctype="text/event-stream; charset=utf-8", delay=0.03):
        self.chunks, self.ctype, self.delay = chunks, ctype, delay


def js(obj, status=200, headers=None):
    return Reply(status, json.dumps(obj).encode(), "application/json", headers)


def media(value):
    return value.split(";")[0].strip().lower()


def pairs(encoded):
    """Decoded `name=value` pairs by name, repeated names in the order they were sent."""
    decoded = sorted(parse_qsl(encoded, keep_blank_values=True), key=lambda pair: pair[0])
    return "&".join(f"{k}={v}" for k, v in decoded)


def encoding_problems(raw, where, allowed):
    """Every character is allowed unencoded, or a percent escape with uppercase hex digits."""
    problems = []
    i = 0
    while i < len(raw):
        c = raw[i]
        if c == "%":
            digits = raw[i + 1 : i + 3]
            if len(digits) != 2 or any(d not in HEX for d in digits):
                problems.append(f"{where}: invalid percent escape {raw[i:i + 3]!r} at offset {i} (needs two uppercase hex digits)")
            i += 3
            continue
        if c not in UNRESERVED and c not in allowed:
            problems.append(f"{where}: character {c!r} at offset {i} must be percent-encoded")
        i += 1
    return problems


def parse_instant(text):
    m = re.fullmatch(r"(\d{4})-(\d\d)-(\d\d)[Tt](\d\d):(\d\d):(\d\d)(?:\.(\d{1,9}))?([Zz]|[+-]\d\d:\d\d)", text)
    if not m:
        return None
    year, month, day, hour, minute, second = (int(g) for g in m.groups()[:6])
    micro = int((m.group(7) or "0").ljust(6, "0")[:6])
    zone = m.group(8)
    if zone in ("Z", "z"):
        tz = timezone.utc
    else:
        sign = 1 if zone[0] == "+" else -1
        tz = timezone(sign * timedelta(hours=int(zone[1:3]), minutes=int(zone[4:6])))
    try:
        return datetime(year, month, day, hour, minute, second, micro, tzinfo=tz)
    except ValueError:
        return None


class Route:
    def __init__(self, op, method, pattern, handler, query=(), required=(), repeat=(), headers=(),
                 content_type=None, body="none", security="none"):
        self.op, self.method, self.regex, self.handler = op, method, re.compile(pattern), handler
        self.query = None if query is None else set(query) | set(required)
        self.required, self.repeat, self.required_headers = set(required), set(repeat), headers
        self.content_type, self.body, self.security = content_type, body, security


class Req:
    def __init__(self, method, raw_path, raw_query, headers, body, match):
        self.method, self.raw_path, self.raw_query = method, raw_path, raw_query
        self.headers, self.body, self.match = headers, body, match
        self.problems = []
        self.query = []
        self._json = MISSING
        self._parsed = False

    def problem(self, text):
        self.problems.append(text)

    def h(self, name, default=""):
        return self.headers.get(name, default)

    def qd(self):
        out = {}
        for k, v in self.query:
            out.setdefault(k, v)
        return out

    def qall(self, name):
        return [v for k, v in self.query if k == name]

    def arg(self, index=1):
        raw = self.match.group(index)
        try:
            return unquote(raw, errors="strict")
        except UnicodeDecodeError:
            self.problem(f"path: segment {raw!r} is not valid percent-encoded UTF-8")
            return ""

    def cookies(self):
        out = {}
        for part in self.h("cookie").split(";"):
            if "=" in part:
                k, v = part.strip().split("=", 1)
                out.setdefault(k, v)
        return out

    def auth(self):
        return "|".join([self.h("authorization"), self.h("x-api-key"), self.qd().get("api_key", "")])

    def json(self):
        if not self._parsed:
            self._parsed = True
            try:
                self._json = json.loads(self.body.decode("utf-8"))
            except (ValueError, UnicodeDecodeError) as error:
                self.problem(f"body: not valid JSON ({error}): {self.body[:200]!r}")
        return self._json

    def expect_json(self, expected):
        got = self.json()
        if got is not MISSING and json.dumps(got, sort_keys=True) != json.dumps(expected, sort_keys=True):
            self.problem(f"body: expected exactly {json.dumps(expected, sort_keys=True)} but got {self.body.decode('utf-8', 'replace')}")

    def multipart(self):
        try:
            boundary = self.h("content-type").split("boundary=")[1].strip('"').encode()
            parts = []
            for raw in self.body.split(b"--" + boundary)[1:-1]:
                head, _, content = raw[2:-2].partition(b"\r\n\r\n")
                lines = [line.decode().split(": ", 1) for line in head.split(b"\r\n") if line]
                headers = {key.lower(): value for key, value in lines}
                disposition = Message()
                disposition["content-disposition"] = headers["content-disposition"]
                name = disposition.get_param("name", header="content-disposition")
                filename = disposition.get_param("filename", "", header="content-disposition")
                parts.append(f"{name}={filename}:{headers.get('content-type', '')}:{content.decode()}")
            return ";".join(parts)
        except Exception as error:  # noqa: BLE001 - any parse failure is a request problem
            self.problem(f"body: malformed multipart/form-data ({error!r})")
            return ""


# ---- handlers --------------------------------------------------------------------------------


def h_list_widgets(req):
    cursor = req.qd().get("cursor")
    if cursor not in WIDGETS:
        return req.problem(f"query: unexpected cursor {cursor!r}")
    ids, nxt = WIDGETS[cursor]
    # `color` is not in the spec: SDKs keep it as an unknown property.
    return js({"data": [{"id": i, "name": i, "color": "red"} for i in ids], "next_cursor": nxt})


def h_list_widget_events(req):
    if req.arg() != "w1":
        req.problem(f"path: widget_id must be 'w1', got {req.arg()!r}")
    if req.qd().get("kind") != "created":
        req.problem(f"query: kind must be 'created', got {req.qd().get('kind')!r}")
    after, before = req.qd().get("starting_after"), req.qd().get("ending_before")
    if before is not None:
        if after is not None:
            return req.problem("query: only one of starting_after and ending_before may be sent")
        if before not in EVENTS_BEFORE:
            return req.problem(f"query: unexpected ending_before {before!r}")
        ids, more = EVENTS_BEFORE[before]
        return js({"data": [{"id": i, "kind": "created"} for i in ids], "has_more": more})
    if after not in EVENTS:
        return req.problem(f"query: unexpected starting_after {after!r}")
    ids, more = EVENTS[after]
    return js({"data": [{"id": i, "kind": "created"} for i in ids], "has_more": more})


def int_param(req, name, default=None):
    value = req.qd().get(name)
    if value is None:
        return default
    if not re.fullmatch(r"-?\d+", value):
        req.problem(f"query: {name} must be an integer, got {value!r}")
        return default
    return int(value)


def h_list_gadgets(req):
    page = int_param(req, "page", 0)
    if page not in GADGETS:
        return req.problem(f"query: unexpected page {page}")
    int_param(req, "per_page")
    return js({"items": [{"id": i} for i in GADGETS[page]], "meta": {"page": page, "total_pages": 2}})


def h_list_records(req):
    offset = int_param(req, "offset", 0)
    int_param(req, "limit")
    if offset not in RECORDS:
        return req.problem(f"query: unexpected offset {offset}")
    return js({"data": [{"id": i} for i in RECORDS[offset]], "total": 3})


def h_status_echo_auth(req):
    return js({"status": req.auth()})


OAUTH_SECRET = "p@ss word"


def h_machine(req):
    """Echoes the credentials, but revokes the first token issued to a client named `*-revoked-*`."""
    token = req.h("authorization").removeprefix("Bearer ")
    if token.startswith("at-") and "-revoked-" in token and token.endswith("-1"):
        return js({"error": "token revoked"}, 401)
    return h_status_echo_auth(req)


def h_oauth_token(req):
    """Client credentials grant (not part of the spec's operations): the scheme `oauth` of the spec."""
    form = parse_qsl(req.body.decode("utf-8", "replace"), keep_blank_values=True)
    fields = dict(form)
    if len(fields) != len(form):
        req.problem(f"body: repeated form fields in {req.body[:200]!r}")
    if fields.get("grant_type") != "client_credentials":
        req.problem(f"body: grant_type must be client_credentials, got {fields.get('grant_type')!r}")
    if fields.get("scope") != "machines.read":
        req.problem(f"body: scope must be machines.read, got {fields.get('scope')!r}")
    authz = req.h("authorization")
    if authz:
        if not authz.startswith("Basic "):
            req.problem(f"security: Authorization must use the Basic scheme, got {authz!r}")
            return None
        if "client_id" in fields or "client_secret" in fields:
            req.problem("body: client credentials must be sent either in the Authorization header or in the body, not both")
        try:
            # RFC 6749 2.3.1: the id and secret are form-encoded before the base64.
            client_id, _, secret = base64.b64decode(authz[6:], validate=True).decode().partition(":")
        except ValueError:
            req.problem(f"security: malformed Basic credentials {authz!r}")
            return None
        client_id, secret = unquote_plus(client_id), unquote_plus(secret)
    else:
        client_id, secret = fields.get("client_id", ""), fields.get("client_secret")
    if not client_id or secret != OAUTH_SECRET:
        return js({"error": "invalid_client"}, 401)
    with LOCK:
        state = ATTEMPTS.setdefault(client_id, {"attempts": 0, "keys": []})
        state["attempts"] += 1
        n = state["attempts"]
    return js({"access_token": f"at-{client_id}-{n}", "token_type": "Bearer", "expires_in": 3600})


def h_stream_events(req):
    topic = req.qd().get("topic", "")
    return Chunked([chunk.replace("TOPIC", topic) for chunk in STREAM])


def h_completion(req):
    request = req.json()
    if not isinstance(request, dict) or not isinstance(request.get("prompt"), str):
        return req.problem(f"body: expected an object with a string prompt, got {req.body[:200]!r}")
    if not request.get("stream"):
        return js({"text": request["prompt"].upper()})
    chunks = [f'data: {{"delta": "{c}", "index": {i}}}\n\n' for i, c in enumerate(request["prompt"])]
    return Chunked(chunks + ["data: [DONE]\n\n"], delay=0.0)


def h_upload_file(req):
    return js({"status": req.multipart()})


def h_upload_content(req):
    if req.arg() != "f1":
        req.problem(f"path: file_id must be 'f1', got {req.arg()!r}")
    return js({"status": f"{req.h('content-type')}:{req.body.decode()}"})


def h_search(req):
    return js({"status": pairs(req.raw_query)})


def h_beta_search(req):
    if req.qd().get("beta") != "true":
        req.problem("query: beta=true must be sent (it is part of the spec path)")
    int_param(req, "limit")
    status = pairs(req.raw_query)
    if "features" in req.headers:
        status += f"|features={req.headers['features']}"
    return js({"status": status})


def h_create_charge(req):
    kind = media(req.h("content-type"))
    return js({"status": f"{kind}|{pairs(req.body.decode())}"})


def h_put_image(req):
    return js({"status": f"{req.arg()}:{req.h('content-type')}:{req.body.decode()}"})


def item_id(req):
    value = req.arg()
    if value != "i1":
        req.problem(f"path: item_id must be 'i1', got {value!r}")
    return value


def h_get_item(req):
    return js({"id": item_id(req), "name": "first", "note": "hi"})


def h_patch_item(req):
    req.expect_json({"name": "renamed"})
    return js({"id": item_id(req), "name": "renamed", "note": None})


def h_delete_item(req):
    item_id(req)
    return Reply(204, b"", None)


def h_nullable_body(req):
    return Reply(200, b"null")


def h_text(req):
    return Reply(200, b"hello text\n", "text/plain; charset=utf-8")


def h_csv(req):
    return Reply(200, b'id,name\n1,alpha\n2,"be,ta"\n', "text/csv; charset=utf-8")


def h_blob(req):
    # Streamed in 8 parts 50 ms apart: longer than a 250 ms timeout as a whole, but no read is.
    return Chunked([BLOB[i : i + 32] for i in range(0, 256, 32)], "application/octet-stream", delay=0.05)


def h_image(req):
    return Reply(200, IMAGE, "image/png")


def attempt(req, key=None):
    sid = req.h("x-scenario-id")
    with LOCK:
        state = ATTEMPTS.setdefault(sid, {"attempts": 0, "keys": []})
        state["attempts"] += 1
        if key is not None:
            state["keys"].append(key)
        return sid, state["attempts"], list(state["keys"])


def h_flaky(req):
    _, n, _ = attempt(req)
    if n == 1:
        return js({"error": "unavailable"}, 503, {"retry-after": "1"})
    return js({"status": f"attempt={n}"})


def h_rate_limited(req):
    _, n, _ = attempt(req)
    if n == 1:
        return js({"error": "slow down"}, 429, {"retry-after": formatdate(time.time() + 2, usegmt=True)})
    return js({"status": f"attempt={n}"})


def h_unavailable(req):
    attempt(req)
    return js({"error": "unavailable"}, 503, {"retry-after": "0"})


def h_idempotent(req):
    req.expect_json({"amount": 5})
    key = req.h("idempotency-key")
    _, n, keys = attempt(req, key)
    if any(k != keys[0] for k in keys):
        req.problem(f"header: Idempotency-Key must be identical across retries, saw {keys}")
        return None
    if n == 1:
        return js({"error": "unavailable"}, 503, {"retry-after": "0"})
    return js({"status": f"attempts={n};key={key}"})


def h_malformed(req):
    return Reply(200, b'{"status": ')


def h_empty_body(req):
    return Reply(200, b"")


def h_extra_fields(req):
    return js({"status": "ok", "extra": 1, "nested": {"a": [1, 2, {"b": None}]}, "list": [1, "x"]})


def h_get_nulls(req):
    return js({"name": None, "tags": ["a", None, "b"], "counts": {"x": 1, "y": None}, "note": None})


def h_echo_nulls(req):
    body = {"name": None, "tags": ["a", None], "counts": {"x": None, "y": 2}}
    req.expect_json(body)
    return js(body)


def h_get_bag(req):
    return js({"id": "b1", "a": 1, "b": 2})


def h_get_labels(req):
    return js({"k": "v", "z": "y"})


def h_cookie_session(req):
    value = req.cookies().get("session_id")
    if value is None:
        req.problem("header: Cookie must carry session_id=<value>")
    return js({"status": value or ""})


def h_cookie_auth(req):
    return js({"status": req.cookies().get("auth_token", "")})


def h_path_segment(req):
    raw = req.match.group(1)
    if "/" in raw:
        req.problem(f"path: segment {raw!r} contains an unencoded '/' (must be %2F)")
    value = req.arg()
    if value not in PATH_CASES:
        req.problem(f"path: decoded segment {value!r} is not one of the scenario cases {sorted(PATH_CASES)}")
        if req.raw_query:
            req.problem(f"path: a raw '?' or '#' leaked into the URL (query {req.raw_query!r}); it must be encoded in the segment")
    return js({"status": value})


def h_query_text(req):
    value = req.qd().get("q", "")
    if value not in QUERY_CASES:
        req.problem(f"query: decoded q {value!r} is not one of the scenario cases {sorted(QUERY_CASES)} (a literal '+' must be sent as %2B)")
    return js({"status": value})


def h_query_multi(req):
    ids = req.qall("ids")
    if ids != ["b", "a", "c"]:
        req.problem(f"query: ids must be sent as ids=b&ids=a&ids=c in that order, got {ids}")
    if req.qd().get("flag") != "true":
        req.problem(f"query: flag must be 'true', got {req.qd().get('flag')!r}")
    return js({"status": ",".join(ids)})


def check_instant(req, where, text):
    parsed = parse_instant(text) if isinstance(text, str) else None
    if parsed is None:
        req.problem(f"{where}: {text!r} is not an RFC 3339 date-time (a literal '+' in a query must be %2B)")
    elif parsed != INSTANT:
        req.problem(f"{where}: instant {text!r} must equal 2024-01-02T03:04:05.250Z")


DATE_BOX = {"at": "2024-01-02T03:04:05.250Z", "day": "2024-01-02"}


def h_datetime_query(req):
    check_instant(req, "query: since", req.qd().get("since"))
    if req.qd().get("day") != "2024-01-02":
        req.problem(f"query: day must be '2024-01-02', got {req.qd().get('day')!r}")
    return js(DATE_BOX)


def h_datetime_body(req):
    body = req.json()
    if body is not MISSING:
        if not isinstance(body, dict) or set(body) != {"at", "day"}:
            req.problem(f"body: expected an object with exactly at and day, got {body!r}")
        else:
            check_instant(req, "body: at", body["at"])
            if body["day"] != "2024-01-02":
                req.problem(f"body: day must be '2024-01-02', got {body['day']!r}")
    return js(DATE_BOX)


def h_big_int(req):
    text = req.body.decode("utf-8", "replace")
    if not re.search(r'"value"\s*:\s*9007199254740993(?![\d.eE])', text):
        req.problem(f"body: value must be the exact integer literal 9007199254740993, got {text}")
    if not re.search(r'"min"\s*:\s*-9223372036854775808(?![\d.eE])', text):
        req.problem(f"body: min must be the exact integer literal -9223372036854775808, got {text}")
    return Reply(200, b'{"value":9007199254740993,"min":-9223372036854775808}')


def h_bytes_echo(req):
    req.expect_json({"data": BYTES_B64})
    return js({"data": BYTES_B64})


def h_bytes_get(req):
    return js({"data": BYTES_B64})


def h_enum(req):
    mode = req.qd().get("mode")
    if mode == "known":
        return js({"kind": "red", "kinds": ["green", "blue"]})
    if mode == "unknown":
        return js({"kind": "magenta", "kinds": ["red", "magenta"]})
    return req.problem(f"query: mode must be 'known' or 'unknown', got {mode!r}")


def h_pages(req):
    cursor = req.qd().get("cursor")
    if cursor is None:
        return js({"data": [{"id": "p1", "name": "p1"}, {"id": "p2", "name": "p2"}], "next_cursor": "c2"})
    if cursor == "c2":
        return js({"error": "page_gone", "code": 409}, 409)
    return req.problem(f"query: unexpected cursor {cursor!r}")


def h_sse(req):
    return Chunked(SSE_CHUNKS)


def h_sse_denied(req):
    return js({"error": "forbidden", "code": 403}, 403)


def h_status(req):
    raw = req.arg()
    if not raw.isdigit() or int(raw) not in STATUS_CODES:
        return req.problem(f"path: code must be one of {sorted(STATUS_CODES)}, got {raw!r}")
    return js({"error": f"status {raw}", "code": int(raw)}, int(raw))


def h_headers(req):
    return js({"status": f"{req.h('x-tenant')}|{req.h('x-trace-id')}"})


S = "/scenarios"
ROUTES = [
    Route("list_widgets", "GET", r"/widgets", h_list_widgets, query=("cursor", "limit"), security="default"),
    Route("list_widget_events", "GET", r"/widgets/([^/]+)/events", h_list_widget_events,
          query=("starting_after", "ending_before"), required=("kind",), security="default"),
    Route("list_gadgets", "GET", r"/gadgets", h_list_gadgets, query=("page", "per_page"), security="default"),
    Route("list_records", "GET", r"/records", h_list_records, query=("offset", "limit"), required=("api_key",), security="query_key"),
    Route("health", "GET", r"/health", h_status_echo_auth),
    Route("create_session", "POST", r"/session", h_status_echo_auth, security="basic"),
    Route("machine_status", "GET", r"/machine", h_machine, security="bearer"),
    Route("oauth_token", "POST", r"/oauth/token", h_oauth_token, content_type="application/x-www-form-urlencoded", body="required", security="client"),
    Route("stream_events", "GET", r"/events/stream", h_stream_events, query=("topic",), security="default"),
    Route("create_completion", "POST", r"/completions", h_completion, content_type="application/json", body="required", security="default"),
    Route("upload_file", "POST", r"/files", h_upload_file, content_type="multipart/form-data", body="required", security="default"),
    Route("upload_content", "PUT", r"/files/([^/]+)/content", h_upload_content,
          content_type="application/octet-stream", body="required", security="default"),
    Route("search", "GET", r"/wire/search", h_search, query=None, security="default"),
    Route("create_charge", "POST", r"/wire/charges", h_create_charge,
          content_type="application/x-www-form-urlencoded", body="optional", security="default"),
    Route("beta_search", "GET", r"/wire/beta", h_beta_search, query=("limit",), required=("beta",), security="default"),
    Route("put_image", "PUT", r"/wire/images/([^/]+)", h_put_image, content_type="image/png", body="required", security="default"),
    Route("get_item", "GET", r"/items/([^/]+)", h_get_item),
    Route("patch_item", "PATCH", r"/items/([^/]+)", h_patch_item, content_type="application/json", body="required"),
    Route("delete_item", "DELETE", r"/items/([^/]+)", h_delete_item),
    Route("nullable_body", "GET", S + r"/nullable-body", h_nullable_body),
    Route("text_plain", "GET", S + r"/text", h_text),
    Route("text_csv", "GET", S + r"/csv", h_csv),
    Route("download_blob", "GET", S + r"/blob", h_blob),
    Route("download_image", "GET", S + r"/image", h_image),
    Route("flaky_service", "GET", S + r"/flaky", h_flaky, headers=("x-scenario-id",)),
    Route("rate_limited", "GET", S + r"/rate-limited", h_rate_limited, headers=("x-scenario-id",)),
    Route("always_unavailable", "GET", S + r"/unavailable", h_unavailable, headers=("x-scenario-id",)),
    Route("idempotent_create", "POST", S + r"/idempotent", h_idempotent,
          headers=("x-scenario-id", "idempotency-key"), content_type="application/json", body="required"),
    Route("malformed_json", "GET", S + r"/malformed", h_malformed),
    Route("empty_body", "GET", S + r"/empty-body", h_empty_body),
    Route("extra_fields", "GET", S + r"/extra-fields", h_extra_fields),
    Route("get_nulls", "GET", S + r"/nulls", h_get_nulls),
    Route("echo_nulls", "POST", S + r"/nulls", h_echo_nulls, content_type="application/json", body="required"),
    Route("get_bag", "GET", S + r"/bag", h_get_bag),
    Route("get_labels", "GET", S + r"/labels", h_get_labels),
    Route("cookie_session", "GET", S + r"/cookie", h_cookie_session, headers=("cookie",)),
    Route("cookie_auth", "GET", S + r"/cookie-auth", h_cookie_auth, security="cookie_key"),
    Route("path_segment", "GET", S + r"/paths/(.*)", h_path_segment),
    Route("query_text", "GET", S + r"/query", h_query_text, required=("q",)),
    Route("query_multi", "GET", S + r"/multi", h_query_multi, required=("ids", "flag"), repeat=("ids",)),
    Route("datetime_query", "GET", S + r"/datetime", h_datetime_query, required=("since", "day")),
    Route("datetime_body", "POST", S + r"/datetime", h_datetime_body, content_type="application/json", body="required"),
    Route("big_int", "POST", S + r"/bigint", h_big_int, content_type="application/json", body="required"),
    Route("bytes_echo", "POST", S + r"/bytes", h_bytes_echo, content_type="application/json", body="required"),
    Route("bytes_get", "GET", S + r"/bytes", h_bytes_get),
    Route("enum_value", "GET", S + r"/enum", h_enum, required=("mode",)),
    Route("list_pages", "GET", S + r"/pages", h_pages, query=("cursor",)),
    Route("sse_chunked", "GET", S + r"/sse", h_sse),
    Route("sse_denied", "GET", S + r"/sse-error", h_sse_denied),
    Route("status_error", "GET", S + r"/status/([^/]+)", h_status),
    Route("tenant_headers", "GET", S + r"/headers", h_headers, headers=("x-tenant",)),
]

# ---- generic checks --------------------------------------------------------------------------


def check_security(req, mode):
    """Adds problems for a wrong scheme; returns a 401 Reply when credentials are missing."""
    authz, key = req.h("authorization"), req.h("x-api-key")
    query_key, cookie_key = req.qd().get("api_key"), req.cookies().get("auth_token")
    unauthorized = js({"error": "unauthorized"}, 401)
    if mode == "none":
        present = [n for n, v in (("Authorization", authz), ("X-API-Key", key), ("api_key query", query_key), ("auth_token cookie", cookie_key)) if v]
        if present:
            req.problem(f"security: operation declares security: [] but the request carries credentials: {present}")
    elif mode == "default":
        if authz and not authz.startswith("Bearer "):
            req.problem(f"security: Authorization must use the Bearer scheme, got {authz!r}")
        if not authz and not key:
            return unauthorized
    elif mode in ("basic", "bearer"):
        scheme = "Basic " if mode == "basic" else "Bearer "
        if authz and not authz.startswith(scheme):
            req.problem(f"security: Authorization must use the {scheme.strip()} scheme, got {authz!r}")
        if not authz:
            return unauthorized
    elif mode == "query_key":
        if query_key != "tok":
            return unauthorized
    elif mode == "cookie_key":
        if not cookie_key:
            return unauthorized
    return None


def check(route, req):
    req.problems += encoding_problems(req.raw_path, "path", PATH_OK)
    req.problems += encoding_problems(req.raw_query, "query", QUERY_OK)
    if req.raw_query:
        for part in req.raw_query.split("&"):
            if not part:
                req.problem("query: empty pair (stray '&')")
                continue
            name, _, value = part.partition("=")
            try:
                req.query.append((unquote_plus(name, errors="strict"), unquote_plus(value, errors="strict")))
            except UnicodeDecodeError:
                req.problem(f"query: {part!r} is not valid percent-encoded UTF-8")
    names = [k for k, _ in req.query]
    if route.query is not None:
        for name in sorted(set(names) - route.query):
            req.problem(f"query: unexpected parameter {name!r}")
        for name in sorted({n for n in names if names.count(n) > 1} - route.repeat):
            req.problem(f"query: parameter {name!r} must not be repeated")
    for name in sorted(route.required - set(names)):
        req.problem(f"query: required parameter {name!r} is missing")
    for name in route.required_headers:
        if not req.h(name):
            req.problem(f"header: required header {name!r} is missing")
    ctype = req.h("content-type")
    if route.body == "none" and req.body:
        req.problem(f"body: operation takes no body but {len(req.body)} bytes were sent")
    if route.body == "required" and not req.body:
        req.problem("body: a request body is required")
    if route.content_type and req.body:
        if media(ctype) != route.content_type:
            req.problem(f"header: content-type must be {route.content_type}, got {ctype!r}")
        elif route.content_type == "multipart/form-data" and "boundary=" not in ctype:
            req.problem("header: multipart content-type needs a boundary parameter")
    return check_security(req, route.security)


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):
        pass

    def read_body(self):
        if self.headers.get("transfer-encoding", "").lower() == "chunked":
            data = b""
            while True:
                size = int(self.rfile.readline().strip(), 16)
                chunk = self.rfile.read(size + 2)[:size]
                if not size:
                    return data
                data += chunk
        return self.rfile.read(int(self.headers.get("content-length", "0")))

    def send_reply(self, reply):
        self.send_response(reply.status)
        self.send_header("x-request-id", "req_mock")
        if reply.ctype:
            self.send_header("content-type", reply.ctype)
        if reply.status != 204:
            self.send_header("content-length", str(len(reply.body)))
        for name, value in reply.headers.items():
            self.send_header(name, value)
        self.end_headers()
        if reply.status != 204:
            self.wfile.write(reply.body)

    def send_chunked(self, stream):
        self.send_response(200)
        self.send_header("content-type", stream.ctype)
        self.send_header("cache-control", "no-cache")
        self.send_header("x-request-id", "req_mock")
        self.send_header("transfer-encoding", "chunked")
        self.end_headers()
        self.wfile.flush()
        for chunk in stream.chunks:
            data = chunk if isinstance(chunk, bytes) else chunk.encode()
            self.wfile.write(f"{len(data):X}\r\n".encode() + data + b"\r\n")
            self.wfile.flush()
            time.sleep(stream.delay)
        self.wfile.write(b"0\r\n\r\n")
        self.wfile.flush()

    def mismatch(self, op, req, status=400, summary="request does not match the spec"):
        received = {
            "method": req.method,
            "path": req.raw_path,
            "query": req.raw_query,
            "headers": dict(req.headers),
            "body": req.body[:500].decode("utf-8", "replace"),
        }
        self.send_reply(js({"error": summary, "operation": op, "problems": req.problems, "received": received}, status))

    def handle_any(self):
        raw_path, _, raw_query = self.path.partition("?")
        headers = {k.lower(): v for k, v in self.headers.items()}
        body = self.read_body()
        if raw_path.startswith("/__server/attempts/"):
            sid = unquote(raw_path.rsplit("/", 1)[1])
            with LOCK:
                state = ATTEMPTS.get(sid, {"attempts": 0, "keys": []})
                return self.send_reply(js({"id": sid, "attempts": state["attempts"], "keys": state["keys"]}))
        candidates = [(r, r.regex.fullmatch(raw_path)) for r in ROUTES]
        candidates = [(r, m) for r, m in candidates if m]
        probe = Req(self.command, raw_path, raw_query, headers, body, None)
        if not candidates:
            probe.problem(f"no operation serves path {raw_path!r}")
            return self.mismatch(None, probe, 404, "unknown path")
        allowed = sorted({r.method for r, _ in candidates})
        chosen = [(r, m) for r, m in candidates if r.method == self.command]
        if not chosen:
            probe.problem(f"method {self.command} not allowed on {raw_path!r}; allowed: {allowed}")
            self.send_response(405)
            data = json.dumps({"error": "method not allowed", "problems": probe.problems, "allowed": allowed}).encode()
            self.send_header("allow", ", ".join(allowed))
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
            return None
        route, match = chosen[0]
        req = Req(self.command, raw_path, raw_query, headers, body, match)
        try:
            early = check(route, req)
            result = route.handler(req) if not req.problems else None
        except Exception:  # noqa: BLE001 - surface mock bugs loudly on stderr
            traceback.print_exc(file=sys.stderr)
            req.problem("mock server crashed while handling this request (see its stderr)")
            return self.mismatch(route.op, req, 500, "mock server error")
        if req.problems:
            return self.mismatch(route.op, req)
        if early is not None:
            return self.send_reply(early)
        if isinstance(result, Chunked):
            return self.send_chunked(result)
        return self.send_reply(result)

    do_GET = do_POST = do_PUT = do_PATCH = do_DELETE = handle_any


if __name__ == "__main__":
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    print(f"http://127.0.0.1:{server.server_address[1]}", flush=True)
    server.serve_forever()

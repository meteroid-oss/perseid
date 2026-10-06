import asyncio
import base64
import io
import itertools
import json
import os
import time
import urllib.parse
import urllib.request
from datetime import date, datetime, timedelta, timezone

from features import (
    APIConnectionError,
    APIResponseValidationError,
    AsyncFeatures,
    AuthenticationError,
    BadRequestError,
    ConflictError,
    Features,
    FeaturesError,
    InternalServerError,
    NotFoundError,
    PermissionDeniedError,
    UnprocessableEntityError,
)
from features.api import StreamingUploadFileBody, Upload
from features.models import (
    UNSET,
    ChargeItemsItem,
    ChargeShipping,
    ChargeShippingAddress,
    CompletionChunk,
    Error,
    Filter,
    FilterAmount,
    Health,
    SearchRange,
)

URL = os.environ["FEATURES_URL"]


_scenario_ids = itertools.count(1)


def scenario_id(name):
    """A scenario id of the server, unique to this run of one scenario."""
    return f"python-{name}-{next(_scenario_ids)}"


def server_state(scenario):
    """What the server saw for a scenario: its attempts and idempotency keys."""
    with urllib.request.urlopen(f"{URL}/__server/attempts/{urllib.parse.quote(scenario, safe='')}") as reply:
        return json.load(reply)


def field(body, name):
    """A field of an error body, decoded into a model or left as JSON."""
    return body[name] if isinstance(body, dict) else getattr(body, name)


def raw_bytes(value):
    """The bytes of a `format: byte` value, which Python keeps as base64 text."""
    return base64.b64decode(value) if isinstance(value, str) else bytes(value)


def raises(kind, call):
    """The error `call()` raises, which must be a `kind`."""
    try:
        call()
    except kind as error:
        return error
    raise AssertionError(f"expected {kind.__name__}")


async def araises(kind, awaitable):
    """The error awaiting `awaitable` raises, which must be a `kind`."""
    try:
        await awaitable
    except kind as error:
        return error
    raise AssertionError(f"expected {kind.__name__}")


def ids(items):
    return [item.id for item in items]


client = Features(api_key="tok", base_url=URL)
assert client.account.check_health().status == "||"
assert client.account.retrieve_machine().status == "Bearer tok||"
assert ids(client.widgets.list()) == ["w1", "w2", "w3"]
assert ids(client.widgets.list_events("w1", kind="created")) == ["e1", "e2", "e3"]
assert ids(client.gadgets.list()) == ["g1", "g2", "g3"]
assert ids(client.records.list()) == ["r1", "r2", "r3"]

page = client.widgets.list()
assert ids(page.items) == ["w1", "w2"] and page.body.next_cursor == "c2"
assert page.has_next_page()
last = page.get_next_page()
assert ids(last.items) == ["w3"] and not last.has_next_page()
assert [ids(p.items) for p in client.gadgets.list().iter_pages()] == [["g1", "g2"], ["g3"]]
assert page.items[0].extra_fields == {"color": "red"} and page.items[0].color == "red"
assert page.items[0].to_dict() == {"id": "w1", "name": "w1", "color": "red"}

raw = client.with_raw_response.widgets.list()
assert raw.status_code == 200 and raw.request_id == "req_mock", raw.headers
assert ids(raw.parse().items) == ["w1", "w2"]
assert client.account.with_raw_response.check_health().headers["x-request-id"] == "req_mock"

os.environ["FEATURES_API_KEY"] = "env-tok"
os.environ["FEATURES_BASE_URL"] = URL
assert Features().account.retrieve_machine().status == "Bearer env-tok||"
del os.environ["FEATURES_API_KEY"], os.environ["FEATURES_BASE_URL"]

basic = Features(base_url=URL, basic_auth=("u", "p"))
assert basic.account.create_session().status == "Basic dTpw||"
keyed = Features(base_url=URL, api_keys={"api_key": "k"})
assert ids(keyed.widgets.list()) == ["w1", "w2", "w3"]
provided = Features(base_url=URL, token_provider=lambda: "fresh")
assert provided.account.retrieve_machine().status == "Bearer fresh||"

try:
    Features(base_url=URL).widgets.list()
    raise AssertionError("expected a 401")
except AuthenticationError as error:
    assert (error.status_code, error.body, error.request_id) == (
        401,
        Error(error="unauthorized"),
        "req_mock",
    ), error
try:
    client.with_options(base_url=URL + "/v0").account.check_health()
    raise AssertionError("expected a 404")
except NotFoundError as error:
    # The strict mock answers an unserved path with what it received.
    assert isinstance(error.body, Error) and error.body.error == "unknown path", error.body
    assert error.body.extra_fields["received"]["path"] == "/v0/health", error.body.extra_fields
try:
    Features(api_key="tok", base_url="http://127.0.0.1:9").account.check_health(max_retries=0)
    raise AssertionError("expected a connection error")
except APIConnectionError as error:
    assert isinstance(error, FeaturesError)

assert client.streaming.create_completion(prompt="ab").text == "AB"
with client.streaming.create_completion_stream(prompt="ab") as stream:
    chunks = list(stream)
    assert stream.last_event is not None and stream.last_event.event == "message"
assert [type(c) for c in chunks] == [CompletionChunk, CompletionChunk]
assert [(c.delta, c.extra_fields) for c in chunks] == [("a", {"index": 0}), ("b", {"index": 1})]
assert client.streaming.create_completion(prompt="x", extra_body={"prompt": "yz"}).text == "YZ"

events = list(client.streaming.retrieve_events_stream(topic="news"))
assert [(e.event, e.data, e.id, e.retry) for e in events] == [
    ("greeting", "news", "1", None),
    ("message", "line1\nline2", "1", None),
    ("message", '{"n": 3}', "3", 1500),
], events
body = StreamingUploadFileBody(
    file=Upload(b"hello", "a.txt", "text/plain"),
    name="doc",
    count=2,
    meta=Health(status="ok"),
    tags=["a", "b"],
)
assert client.streaming.upload_file(body).status == (
    'count=::2;file=a.txt:text/plain:hello;meta=:application/json:{"status": "ok"}'
    ";name=::doc;tags=::a;tags=::b"
)
assert client.streaming.upload_content("f1", b"raw").status == "application/octet-stream:raw"
assert client.streaming.upload_content("f1", io.BytesIO(b"io")).status == "application/octet-stream:io"

searched = client.wire.search(
    filter=Filter(status="open", amount=FilterAmount(gte=5)),
    expand=["a", "b"],
    metadata={"k": "v"},
    ids=["x", "y"],
    tags=["t1", "t2"],
    range=SearchRange(gte=1, lt=9),
)
assert searched.status == (
    "expand[]=a&expand[]=b&filter[amount][gte]=5&filter[status]=open&ids=x&ids=y"
    "&metadata[k]=v&range[gte]=1&range[lt]=9&tags=t1,t2"
), searched
assert client.wire.search(ids="z").status == "ids=z"
assert client.wire.search(ids="z", extra_query={"debug": True}).status == "debug=true&ids=z"
charged = client.wire.create_charge(
    amount=100,
    capture=True,
    metadata={"order": "7"},
    items=[ChargeItemsItem(price="p1", quantity=2), ChargeItemsItem(price="p2")],
    expand=["customer"],
    statuses=["a", "b"],
    codes=["c1", "c2"],
    shipping=ChargeShipping(address=ChargeShippingAddress(line1="1 Main", city="Paris")),
)
assert charged.status == (
    "application/x-www-form-urlencoded|amount=100&capture=true&codes=c1,c2&expand[]=customer"
    "&items[0][price]=p1&items[0][quantity]=2&items[1][price]=p2&metadata[order]=7"
    "&shipping[address][city]=Paris&shipping[address][line1]=1 Main&statuses=a&statuses=b"
)
assert client.wire.create_charge(amount=1, extra_body={"note": "n"}).status == (
    "application/x-www-form-urlencoded|amount=1&note=n"
)
assert client.wire.beta_search(limit=2, features=["x", "y"]).status == "beta=true&limit=2|features=x,y"
assert client.wire.update_image("42", b"png").status == "42:image/png:png"


# Scenarios of tests/features/SCENARIOS.md.

# Items: methods and empty responses.
item = client.items.retrieve("i1")
assert (item.id, item.name, item.note) == ("i1", "first", "hi"), item
patched = client.items.update("i1", name="renamed")
assert (patched.name, patched.note) == ("renamed", None), patched
assert client.items.delete("i1") is None

# Content and decoding.
assert client.content.retrieve_scenarios_nullable_body() is None
assert client.content.retrieve_scenarios_text() == "hello text\n"
assert client.content.retrieve_scenarios_csv() == 'id,name\n1,alpha\n2,"be,ta"\n'
assert client.content.download_blob() == bytes(range(256))
assert client.content.download_image() == b"\x89PNG\r\n\x1a\n" + b"\x00\x01\xfe\xff" * 4
for malformed in (client.content.retrieve_scenarios_malformed, client.content.retrieve_scenarios_empty_body):
    decode_error = raises(APIResponseValidationError, malformed)
    assert isinstance(decode_error, FeaturesError) and decode_error.status_code == 200, decode_error
assert raises(APIResponseValidationError, client.content.retrieve_scenarios_malformed).raw_body == b'{"status": '
assert raises(APIResponseValidationError, client.content.retrieve_scenarios_empty_body).raw_body == b""
extras = client.content.list_scenarios_extra_fields()
assert extras.status == "ok" and extras.extra_fields["extra"] == 1, extras
nulls = client.content.list_scenarios_nulls()
assert nulls.name is None and nulls.note is None, nulls
assert nulls.tags == ["a", None, "b"] and nulls.counts == {"x": 1, "y": None}, nulls
echoed = client.content.create_scenarios_null(name=None, tags=["a", None], counts={"x": None, "y": 2})
assert (echoed.name, echoed.tags, echoed.counts) == (None, ["a", None], {"x": None, "y": 2}), echoed
assert echoed.note is UNSET, echoed
assert echoed.to_dict() == {"name": None, "tags": ["a", None], "counts": {"x": None, "y": 2}}, echoed
bag = client.content.retrieve_scenarios_bag()
assert bag.id == "b1" and bag.extra_fields == {"a": 1, "b": 2}, bag
assert client.content.list_scenarios_labels() == {"k": "v", "z": "y"}
known = client.content.retrieve_scenarios_enum(mode="known")
assert known.kind == "red" and known.kinds == ["green", "blue"], known
unknown = client.content.retrieve_scenarios_enum(mode="unknown")
assert unknown.kind == "magenta" and unknown.kinds == ["red", "magenta"], unknown

# Encoding.
big = client.encoding.scenarios_bigint(value=9007199254740993, min=-9223372036854775808)
assert (big.value, big.min) == (9007199254740993, -9223372036854775808), big
HELLO = b"hello\xfb\xff\xfe"
assert raw_bytes(client.encoding.create_scenarios_byte(data="aGVsbG/7//4=").data) == HELLO
assert raw_bytes(client.encoding.list_scenarios_bytes().data) == HELLO
INSTANT = datetime(2024, 1, 2, 3, 4, 5, 250000, tzinfo=timezone.utc)
for zone in (timezone.utc, timezone(timedelta(hours=2)), timezone(timedelta(hours=-5, minutes=-30))):
    at = INSTANT.astimezone(zone)
    stamped = client.encoding.retrieve_scenarios_datetime(since=at, day=date(2024, 1, 2))
    assert stamped.at == INSTANT and stamped.day == date(2024, 1, 2), (zone, stamped)
    posted = client.encoding.scenarios_datetime(at=at, day=date(2024, 1, 2))
    assert posted.at == INSTANT and posted.day == date(2024, 1, 2), (zone, posted)
for value in ("plain", "sp ace", "sl/ash", "q?mark", "per%cent", "ha#sh", "lit%25eral", "a+b", "héllo wörld ✓"):
    assert client.encoding.retrieve_scenario_path(value).status == value, value
for value in ("plain", "sp ace", "a&b=c+d", "100%", "slash/qm?", "héllo wörld ✓"):
    assert client.encoding.retrieve_scenarios_query(q=value).status == value, value
assert client.encoding.retrieve_scenarios_multi(ids=["b", "a", "c"], flag=True).status == "b,a,c"
assert client.encoding.list_scenarios_headers(x_tenant="acme").status == "acme|"
assert client.encoding.list_scenarios_headers(x_tenant="acme", x_trace_id="t1").status == "acme|t1"

# Cookies.
assert client.cookies.retrieve_scenarios_cookie(session_id="abc123").status == "abc123"
cookied = Features(base_url=URL, api_keys={"api_key_cookie": "ck1"})
assert cookied.cookies.retrieve_scenarios_cookie_auth().status == "ck1"
raises(AuthenticationError, Features(base_url=URL).cookies.retrieve_scenarios_cookie_auth)

# OAuth2 client credentials.
OAUTH_SECRET = "p@ss word"
oauth = Features(base_url=URL, client_id="python-oauth", client_secret=OAUTH_SECRET)
assert oauth.account.retrieve_machine().status == "Bearer at-python-oauth-1||"
assert oauth.account.retrieve_machine().status == "Bearer at-python-oauth-1||"
assert server_state("python-oauth")["attempts"] == 1, "the token is cached"
in_body = Features(
    base_url=URL, client_id="python-oauth-body", client_secret=OAUTH_SECRET, oauth_client_auth="body"
)
assert in_body.account.retrieve_machine().status == "Bearer at-python-oauth-body-1||"
revoked = Features(base_url=URL, client_id="python-oauth-revoked", client_secret=OAUTH_SECRET, max_retries=0)
assert revoked.account.retrieve_machine().status == "Bearer at-python-oauth-revoked-2||"
assert server_state("python-oauth-revoked")["attempts"] == 2, "a rejected token is replaced"
invalid = Features(base_url=URL, client_id="python-oauth-bad", client_secret="wrong", max_retries=0)
invalid_error = raises(AuthenticationError, invalid.account.retrieve_machine)
assert field(invalid_error.body, "error") == "invalid_client", invalid_error
os.environ["FEATURES_CLIENT_ID"], os.environ["FEATURES_CLIENT_SECRET"] = "python-oauth-env", OAUTH_SECRET
assert Features(base_url=URL).account.retrieve_machine().status == "Bearer at-python-oauth-env-1||"
del os.environ["FEATURES_CLIENT_ID"], os.environ["FEATURES_CLIENT_SECRET"]
token_wins = Features(base_url=URL, api_key="tok", client_id="python-oauth", client_secret=OAUTH_SECRET)
assert token_wins.account.retrieve_machine().status == "Bearer tok||"

# Retries and idempotency.
flaky = scenario_id("flaky")
started = time.monotonic()
assert client.retries.retrieve_scenarios_flaky(x_scenario_id=flaky).status == "attempt=2"
assert time.monotonic() - started >= 0.9, "Retry-After: 1 is waited for"
assert server_state(flaky)["attempts"] == 2
unretried = scenario_id("flaky-once")
flaky_error = raises(
    InternalServerError,
    lambda: client.retries.retrieve_scenarios_flaky(x_scenario_id=unretried, max_retries=0),
)
assert flaky_error.status_code == 503 and field(flaky_error.body, "error") == "unavailable", flaky_error
assert server_state(unretried)["attempts"] == 1
limited = scenario_id("rate-limited")
started = time.monotonic()
assert client.retries.retrieve_scenarios_rate_limited(x_scenario_id=limited).status == "attempt=2"
assert time.monotonic() - started >= 0.5, "an HTTP-date Retry-After is waited for"
assert server_state(limited)["attempts"] == 2
down = scenario_id("unavailable")
down_error = raises(
    InternalServerError, lambda: client.retries.retrieve_scenarios_unavailable(x_scenario_id=down, max_retries=2)
)
assert down_error.status_code == 503, down_error
assert server_state(down)["attempts"] == 3
idempotent = scenario_id("idempotent")
created = client.retries.scenarios_idempotent(amount=5, x_scenario_id=idempotent, idempotency_key="idem-1")
assert created.status == "attempts=2;key=idem-1", created
assert server_state(idempotent)["keys"] == ["idem-1", "idem-1"]

# Errors and pagination.
STATUS_ERRORS = {
    400: BadRequestError,
    401: AuthenticationError,
    403: PermissionDeniedError,
    404: NotFoundError,
    409: ConflictError,
    422: UnprocessableEntityError,
}
sent = []


def count_requests(request, call_next):
    sent.append(request)
    return call_next(request)


counted = Features(api_key="tok", base_url=URL, middleware=[count_requests])
for code, kind in STATUS_ERRORS.items():
    sent.clear()
    status_error = raises(kind, lambda: counted.errors.retrieve_scenario_status(code))
    assert type(status_error) is kind and status_error.status_code == code, status_error
    assert status_error.body == Error(error=f"status {code}", code=code), status_error.body
    assert status_error.request_id == "req_mock" and len(sent) == 1, (code, len(sent))
pages_seen = []


def drain_pages():
    for widget in client.errors.list_scenarios_pages():
        pages_seen.append(widget.id)


gone = raises(ConflictError, drain_pages)
assert pages_seen == ["p1", "p2"], pages_seen
assert gone.status_code == 409 and field(gone.body, "error") == "page_gone" and field(gone.body, "code") == 409, gone

# Streaming.
with client.streaming.retrieve_scenarios_sse() as sse:
    ticks = [(e.event, e.data, e.id, e.retry) for e in sse]
assert ticks == [
    ("message", "first", None, None),
    ("tick", "line1\nline2", "7", None),
    ("message", '{"n": 3}', "7", 2500),
    ("message", "tail", "7", None),
], ticks
denied = raises(PermissionDeniedError, client.streaming.retrieve_scenarios_sse_error)
assert denied.status_code == 403 and field(denied.body, "error") == "forbidden" and field(denied.body, "code") == 403, denied

async def main():
    async def fresh():
        return "async-fresh"

    async with AsyncFeatures(api_key="tok", base_url=URL) as client:
        assert [w.id async for w in client.widgets.list()] == ["w1", "w2", "w3"]
        assert [g.id async for g in client.gadgets.list()] == ["g1", "g2", "g3"]
        page = await client.widgets.list()
        assert ids(page.items) == ["w1", "w2"] and page.has_next_page()
        assert ids((await page.get_next_page()).items) == ["w3"]
        raw = await client.with_raw_response.widgets.list()
        assert raw.request_id == "req_mock" and ids(raw.parse().items) == ["w1", "w2"]
        events = [e async for e in await client.streaming.retrieve_events_stream(topic="async")]
        assert [e.data for e in events] == ["async", "line1\nline2", '{"n": 3}']
        stream = await client.streaming.create_completion_stream(prompt="xyz", max_retries=0)
        assert [c.delta async for c in stream] == ["x", "y", "z"]
        assert (await client.streaming.upload_content("f1", b"a")).status.endswith(":a")
        # The scenarios again, through the async client.
        assert (await client.items.retrieve("i1")).name == "first"
        assert (await client.items.update("i1", name="renamed")).note is None
        assert await client.items.delete("i1") is None
        assert await client.content.retrieve_scenarios_nullable_body() is None
        assert await client.content.retrieve_scenarios_text() == "hello text\n"
        assert await client.content.download_blob() == bytes(range(256))
        await araises(APIResponseValidationError, client.content.retrieve_scenarios_malformed())
        await araises(APIResponseValidationError, client.content.retrieve_scenarios_empty_body())
        assert (await client.content.list_scenarios_extra_fields()).status == "ok"
        big = await client.encoding.scenarios_bigint(value=9007199254740993, min=-9223372036854775808)
        assert (big.value, big.min) == (9007199254740993, -9223372036854775808), big
        for value in ("sp ace", "sl/ash", "q?mark", "per%cent", "héllo wörld ✓"):
            assert (await client.encoding.retrieve_scenario_path(value)).status == value, value
        assert (await client.encoding.retrieve_scenarios_query(q="a&b=c+d")).status == "a&b=c+d"
        flaky = scenario_id("async-flaky")
        started = time.monotonic()
        assert (await client.retries.retrieve_scenarios_flaky(x_scenario_id=flaky)).status == "attempt=2"
        assert time.monotonic() - started >= 0.9
        assert server_state(flaky)["attempts"] == 2
        idempotent = scenario_id("async-idempotent")
        created = await client.retries.scenarios_idempotent(
            amount=5, x_scenario_id=idempotent, idempotency_key="idem-1"
        )
        assert created.status == "attempts=2;key=idem-1" and server_state(idempotent)["keys"] == ["idem-1"] * 2
        for code, kind in {403: PermissionDeniedError, 404: NotFoundError, 422: UnprocessableEntityError}.items():
            status_error = await araises(kind, client.errors.retrieve_scenario_status(code))
            assert status_error.status_code == code and field(status_error.body, "code") == code, status_error
        seen = []
        try:
            async for widget in client.errors.list_scenarios_pages():
                seen.append(widget.id)
            raise AssertionError("expected a 409")
        except ConflictError as error:
            assert error.status_code == 409 and seen == ["p1", "p2"], (error, seen)
        async with await client.streaming.retrieve_scenarios_sse() as sse:
            ticks = [(e.event, e.data, e.id, e.retry) async for e in sse]
        assert ticks == [
            ("message", "first", None, None),
            ("tick", "line1\nline2", "7", None),
            ("message", '{"n": 3}', "7", 2500),
            ("message", "tail", "7", None),
        ], ticks
        await araises(PermissionDeniedError, client.streaming.retrieve_scenarios_sse_error())
        # Cancelling a call that waits to retry stops it: the server sees one attempt only.
        limited = scenario_id("async-cancel")
        try:
            await asyncio.wait_for(client.retries.retrieve_scenarios_rate_limited(x_scenario_id=limited), 0.4)
            raise AssertionError("expected a timeout")
        except asyncio.TimeoutError:
            pass
        await asyncio.sleep(0.1)
        assert server_state(limited)["attempts"] == 1
    async with AsyncFeatures(base_url=URL, token_provider=fresh) as client:
        assert (await client.account.retrieve_machine()).status == "Bearer async-fresh||"
    async with AsyncFeatures(base_url=URL, client_id="python-oauth-async", client_secret=OAUTH_SECRET) as client:
        first, second = await asyncio.gather(client.account.retrieve_machine(), client.account.retrieve_machine())
        assert first.status == second.status == "Bearer at-python-oauth-async-1||"
        assert server_state("python-oauth-async")["attempts"] == 1, "concurrent calls share one token request"
    async with AsyncFeatures(
        base_url=URL, client_id="python-oauth-async-revoked", client_secret=OAUTH_SECRET
    ) as client:
        assert (await client.account.retrieve_machine()).status == "Bearer at-python-oauth-async-revoked-2||"


asyncio.run(main())
print("python smoke test passed")

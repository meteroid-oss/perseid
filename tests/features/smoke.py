import asyncio
import io
import os

from features import (
    APIConnectionError,
    AsyncFeatures,
    AuthenticationError,
    Features,
    FeaturesError,
    NotFoundError,
)
from features.api import StreamingUploadFileBody, Upload
from features.models import (
    ChargeItemsItem,
    ChargeShipping,
    ChargeShippingAddress,
    CompletionChunk,
    Filter,
    FilterAmount,
    Health,
    SearchRange,
)

URL = os.environ["FEATURES_URL"]


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
        {"error": "unauthorized"},
        "req_mock",
    ), error
try:
    client.with_options(base_url=URL + "/v0").account.check_health()
    raise AssertionError("expected a 404")
except NotFoundError as error:
    assert error.body == {"error": "/v0/health"}, error.body
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
assert client.wire.beta_search(limit=2, features="x,y").status == "beta=true&limit=2|features=x,y"
assert client.wire.update_image("42", b"png").status == "42:image/png:png"


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
    async with AsyncFeatures(base_url=URL, token_provider=fresh) as client:
        assert (await client.account.retrieve_machine()).status == "Bearer async-fresh||"


asyncio.run(main())
print("python smoke test passed")

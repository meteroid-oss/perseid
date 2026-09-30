import asyncio
import io
import os

from features import Features, FeaturesAsync
from features.api import FeaturesOptions, StreamingUploadFileBody, Upload
from features.models import (
    Charge,
    ChargeItemsItem,
    ChargeShipping,
    ChargeShippingAddress,
    Filter,
    FilterAmount,
    Health,
    SearchRange,
)

URL = os.environ["FEATURES_URL"]


def options(**kwargs):
    return FeaturesOptions(server_url=URL, **kwargs)


def ids(items):
    return [item.id for item in items]


client = Features("tok", options())
assert client.account.health().status == "||"
assert client.account.machine_status().status == "Bearer tok||"
assert ids(client.widgets.list_widgets_iter()) == ["w1", "w2", "w3"]
assert ids(client.widgets.list_widget_events_iter("w1", kind="created")) == ["e1", "e2", "e3"]
assert ids(client.gadgets.list_gadgets_iter()) == ["g1", "g2", "g3"]
assert ids(client.records.list_records_iter()) == ["r1", "r2", "r3"]

basic = Features(None, options(basic_auth=("u", "p")))
assert basic.account.create_session().status == "Basic dTpw||"
keyed = Features(None, options(api_keys={"api_key": "k"}))
assert ids(keyed.widgets.list_widgets_iter()) == ["w1", "w2", "w3"]
provided = Features(None, options(token_provider=lambda: "fresh"))
assert provided.account.machine_status().status == "Bearer fresh||"

events = list(client.streaming.stream_events(topic="news"))
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
charge = Charge(
    amount=100,
    capture=True,
    metadata={"order": "7"},
    items=[ChargeItemsItem(price="p1", quantity=2), ChargeItemsItem(price="p2")],
    expand=["customer"],
    statuses=["a", "b"],
    codes=["c1", "c2"],
    shipping=ChargeShipping(address=ChargeShippingAddress(line1="1 Main", city="Paris")),
)
assert client.wire.create_charge(charge).status == (
    "application/x-www-form-urlencoded|amount=100&capture=true&codes=c1,c2&expand[]=customer"
    "&items[0][price]=p1&items[0][quantity]=2&items[1][price]=p2&metadata[order]=7"
    "&shipping[address][city]=Paris&shipping[address][line1]=1 Main&statuses=a&statuses=b"
)
assert client.wire.create_charge().status == "|"
assert client.wire.beta_search(limit=2, features="x,y").status == "beta=true&limit=2|features=x,y"
assert client.wire.put_image("42", b"png").status == "42:image/png:png"


async def main():
    async def fresh():
        return "async-fresh"

    async with FeaturesAsync("tok", options()) as client:
        assert [w.id async for w in client.widgets.list_widgets_iter()] == ["w1", "w2", "w3"]
        assert [g.id async for g in client.gadgets.list_gadgets_iter()] == ["g1", "g2", "g3"]
        events = [e async for e in await client.streaming.stream_events(topic="async")]
        assert [e.data for e in events] == ["async", "line1\nline2", '{"n": 3}']
        assert (await client.streaming.upload_content("f1", b"a")).status.endswith(":a")
    async with FeaturesAsync(None, options(token_provider=fresh)) as client:
        assert (await client.account.machine_status()).status == "Bearer async-fresh||"


asyncio.run(main())
print("python smoke test passed")

import asyncio
import io
import os

from features import Features, FeaturesAsync
from features.api import FeaturesOptions, StreamingUploadFileBody, Upload
from features.models import Health

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
    file=Upload(b"hello", "a.txt", "text/plain"), name="doc", count=2, meta=Health(status="ok")
)
assert client.streaming.upload_file(body).status == (
    'count=::2;file=a.txt:text/plain:hello;meta=:application/json:{"status": "ok"};name=::doc'
)
assert client.streaming.upload_content("f1", b"raw").status == "application/octet-stream:raw"
assert client.streaming.upload_content("f1", io.BytesIO(b"io")).status == "application/octet-stream:io"


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

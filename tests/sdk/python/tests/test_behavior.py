"""Client behavior of the SDK generated from tests/fixtures/features.yaml, on an in-memory transport.

The smoke test (tests/features/smoke.py) drives the same SDK against the strict mock server; these
tests cover what that server cannot show: the exact requests, odd responses, retry timing and
cancellation.
"""

import asyncio
import email.utils
import importlib
import json
import logging
import os
import re
import tempfile
import time
import unittest
import urllib.parse
from datetime import datetime, timedelta, timezone
from pathlib import Path
from unittest import mock

import httpx

from _support import generate_fixture, import_package, instant

features = import_package(generate_fixture("features.yaml", "Features", settings="idempotency_keys = true\n"))
models = features.models

# Credentials and the base URL come from the arguments of the tests only.
os.environ.pop("FEATURES_API_KEY", None)
os.environ.pop("FEATURES_BASE_URL", None)
# Retries without backoff, to keep tests fast.
features.api.common.INITIAL_RETRY_DELAY = 0.0

BASE = "https://features.test/api/v2"
HEALTH = {"status": "ok"}
PAGE = {"data": [{"id": "p1", "name": "p1"}, {"id": "p2", "name": "p2"}], "next_cursor": "c2"}
ITEM = {"id": "i1", "name": "first", "note": "hi"}

PATH_SEGMENTS = {
    "plain": ("plain",),
    "sp ace": ("sp%20ace",),
    "sl/ash": ("sl%2Fash",),
    "q?mark": ("q%3Fmark",),
    "per%cent": ("per%25cent",),
    "ha#sh": ("ha%23sh",),
    "lit%25eral": ("lit%2525eral",),
    "a+b": ("a%2Bb", "a+b"),
    "héllo wörld ✓": ("h%C3%A9llo%20w%C3%B6rld%20%E2%9C%93",),
    "a\\b": ("a%5Cb",),
    "%2e%2e": ("%252e%252e",),
    "a/../b": ("a%2F..%2Fb",),
    # A dot segment would be resolved by the URL, leaving the resource path.
    "..": ("%2E%2E",),
    ".": ("%2E",),
}
QUERY_VALUES = [
    "plain",
    "sp ace",
    "a&b=c+d",
    "100%",
    "slash/qm?",
    "héllo wörld ✓",
    "x#y",
    "[brackets]",
    "",
]


def body_get(body: object, key: str) -> object:
    """A field of an error body, whether it was decoded into a model or left as JSON."""
    return body[key] if isinstance(body, dict) else getattr(body, key)


class Recorder:
    """A transport handler that keeps the requests and answers with `responses` in turn."""

    def __init__(self, *responses: httpx.Response) -> None:
        self.requests: list[httpx.Request] = []
        self.headers: list[dict[str, str]] = []
        self.queue = list(responses)

    def __call__(self, request: httpx.Request) -> httpx.Response:
        self.requests.append(request)
        # A retry sends the same request again, with an updated header.
        self.headers.append(dict(request.headers))
        return self.queue.pop(0) if len(self.queue) > 1 else self.queue[0]


def sync_client(handler, **options):
    """A client of `handler`, with a token."""
    options = {"api_key": "tok", "base_url": BASE, **options}
    transport = httpx.MockTransport(handler)
    return features.Features(timeout=5, http_client=httpx.Client(transport=transport), **options)


def async_client(handler, **options):
    """An async client of `handler`, with a token."""
    options = {"api_key": "tok", "base_url": BASE, **options}
    transport = httpx.MockTransport(handler)
    return features.AsyncFeatures(
        timeout=5, http_client=httpx.AsyncClient(transport=transport), **options
    )


def unavailable(after: str = "0") -> httpx.Response:
    return httpx.Response(503, json={"error": "unavailable"}, headers={"retry-after": after})


class PathAndQueryTest(unittest.TestCase):
    def test_path_segments_are_percent_encoded(self) -> None:
        for value, allowed in PATH_SEGMENTS.items():
            with self.subTest(value=value):
                handler = Recorder(httpx.Response(200, json={"status": value}))
                with sync_client(handler) as api:
                    self.assertEqual(api.encoding.retrieve_scenario_path(value).status, value)
                request = handler.requests[0]
                prefix = b"/api/v2/scenarios/paths/"
                self.assertTrue(request.url.raw_path.startswith(prefix), request.url.raw_path)
                segment = request.url.raw_path[len(prefix) :].decode()
                self.assertIn(segment, allowed)
                self.assertEqual(request.url.query, b"", "a '?' must not start a query")
                self.assertEqual(request.url.fragment, "", "a '#' must not start a fragment")

    def test_path_parameters_of_other_operations_are_escaped_too(self) -> None:
        handler = Recorder(httpx.Response(200, json=ITEM), httpx.Response(204))
        with sync_client(handler) as api:
            api.items.retrieve("a/b?c")
            api.items.delete("..")
        self.assertEqual(handler.requests[0].url.raw_path, b"/api/v2/items/a%2Fb%3Fc")
        self.assertEqual(handler.requests[1].url.raw_path, b"/api/v2/items/%2E%2E")

    def test_dot_segments_of_styled_path_parameters_are_encoded(self) -> None:
        encode = features.api.common.encode_path_param
        self.assertEqual(encode("id", "", "label", False), "%2E")
        self.assertEqual(encode("id", ".", "label", False), "%2E%2E")
        self.assertEqual(encode("id", "a", "label", False), ".a")

    def test_api_key_cookie_keeps_cookie_octets(self) -> None:
        octets = features.api._auth._COOKIE_OCTETS
        quote = urllib.parse.quote
        self.assertEqual(quote("abc/def+ghi==", safe=octets), "abc/def+ghi==")
        self.assertEqual(quote('a b,c;d"e\\f', safe=octets), "a%20b%2Cc%3Bd%22e%5Cf")

    def test_query_values_survive_encoding(self) -> None:
        for value in QUERY_VALUES:
            with self.subTest(value=value):
                handler = Recorder(httpx.Response(200, json={"status": value}))
                with sync_client(handler) as api:
                    api.encoding.retrieve_scenarios_query(q=value)
                raw = handler.requests[0].url.query.decode()
                self.assertEqual(urllib.parse.parse_qsl(raw, keep_blank_values=True), [("q", value)])
                for escape in re.findall(r"%[0-9A-Fa-f]{2}", raw):
                    self.assertEqual(escape, escape.upper())
                self.assertNotIn("#", raw)

    def test_a_list_query_is_exploded_in_order(self) -> None:
        handler = Recorder(httpx.Response(200, json={"status": "b,a,c"}))
        with sync_client(handler) as api:
            api.encoding.retrieve_scenarios_multi(ids=["b", "a", "c"], flag=True)
        pairs = urllib.parse.parse_qsl(handler.requests[0].url.query.decode())
        self.assertEqual(pairs, [("ids", "b"), ("ids", "a"), ("ids", "c"), ("flag", "true")])

    def test_dates_and_times_are_sent_as_rfc_3339(self) -> None:
        want = instant("2024-01-02T03:04:05.250Z")
        handler = Recorder(httpx.Response(200, json={"at": "2024-01-02T03:04:05.250Z", "day": "2024-01-02"}))
        zones = (timezone.utc, timezone(timedelta(hours=1)), timezone(timedelta(hours=-5, minutes=-30)))
        with sync_client(handler) as api:
            for zone in zones:
                at = datetime(2024, 1, 2, 3, 4, 5, 250000, tzinfo=timezone.utc).astimezone(zone)
                api.encoding.retrieve_scenarios_datetime(since=at, day="2024-01-02")
                api.encoding.scenarios_datetime(at=at, day="2024-01-02")
        for request in handler.requests[0::2]:
            raw = request.url.query.decode()
            self.assertNotIn("+", raw, "a literal '+' in a query is a space")
            query = dict(urllib.parse.parse_qsl(raw))
            self.assertEqual(instant(query["since"]), want, query["since"])
            self.assertEqual(query["day"], "2024-01-02")
        for request in handler.requests[1::2]:
            body = json.loads(request.content)
            self.assertEqual(set(body), {"at", "day"})
            self.assertEqual(instant(body["at"]), want, body["at"])

    def test_int64_values_are_sent_as_exact_integer_literals(self) -> None:
        reply = httpx.Response(
            200, content=b'{"value":9007199254740993,"min":-9223372036854775808}',
            headers={"content-type": "application/json"},
        )
        handler = Recorder(reply)
        with sync_client(handler) as api:
            box = api.encoding.scenarios_bigint(value=9007199254740993, min=-9223372036854775808)
        text = handler.requests[0].content.decode()
        self.assertRegex(text, r'"value":\s*9007199254740993(?![\d.eE])')
        self.assertRegex(text, r'"min":\s*-9223372036854775808(?![\d.eE])')
        self.assertEqual((box.value, box.min), (9007199254740993, -9223372036854775808))

    def test_base_url_path_prefix_is_kept(self) -> None:
        handler = Recorder(httpx.Response(200, json=ITEM))
        for base in (BASE, BASE + "/", "https://features.test"):
            with self.subTest(base=base):
                handler.requests.clear()
                with sync_client(handler, base_url=base) as api:
                    api.items.retrieve("i1")
                    api.with_options(base_url="https://other.test/p/q/").items.retrieve("i1")
                prefix = urllib.parse.urlsplit(base).path.rstrip("/")
                self.assertEqual(handler.requests[0].url.raw_path.decode(), f"{prefix}/items/i1")
                self.assertEqual(handler.requests[1].url.raw_path, b"/p/q/items/i1")
                self.assertEqual(handler.requests[1].url.host, "other.test")

    def test_a_query_of_the_spec_path_is_kept_next_to_the_parameters(self) -> None:
        handler = Recorder(httpx.Response(200, json={"status": "beta=true&limit=2"}))
        with sync_client(handler) as api:
            api.wire.beta_search(limit=2)
        request = handler.requests[0]
        self.assertEqual(request.url.path, "/api/v2/wire/beta")
        self.assertEqual(
            sorted(urllib.parse.parse_qsl(request.url.query.decode())), [("beta", "true"), ("limit", "2")]
        )


class HeadersAndCredentialsTest(unittest.TestCase):
    def test_required_and_optional_headers(self) -> None:
        handler = Recorder(httpx.Response(200, json={"status": "acme|"}))
        with sync_client(handler) as api:
            api.encoding.list_scenarios_headers(x_tenant="acme")
            api.encoding.list_scenarios_headers(x_tenant="acme", x_trace_id="t1")
            with self.assertRaises(TypeError):
                api.encoding.list_scenarios_headers()  # type: ignore[call-arg]
        first, second = handler.requests
        self.assertEqual(first.headers["x-tenant"], "acme")
        self.assertNotIn("x-trace-id", first.headers)
        self.assertEqual((second.headers["x-tenant"], second.headers["x-trace-id"]), ("acme", "t1"))
        self.assertEqual(len(handler.requests), 2, "the call without the header is never sent")

    def test_operations_without_security_send_no_credentials(self) -> None:
        handler = Recorder(httpx.Response(200, json=ITEM), httpx.Response(200, json=HEALTH))
        with sync_client(handler, api_keys={"api_key": "k"}, token_provider=lambda: "fresh") as api:
            api.items.retrieve("i1")
            api.cookies.retrieve_scenarios_cookie(session_id="s1")
        for request in handler.requests:
            for name in ("authorization", "x-api-key"):
                self.assertNotIn(name, request.headers)
            self.assertNotIn("api_key", request.url.params)
        self.assertEqual(handler.requests[1].headers["cookie"], "session_id=s1")

    def test_default_security_sends_the_credentials(self) -> None:
        handler = Recorder(httpx.Response(200, json={"data": [], "next_cursor": None}))
        with sync_client(handler) as api:
            api.widgets.list()
        self.assertEqual(handler.requests[0].headers["authorization"], "Bearer tok")
        handler.requests.clear()
        with sync_client(handler, api_key=None, api_keys={"api_key": "k"}) as api:
            api.widgets.list()
        self.assertEqual(handler.requests[0].headers["x-api-key"], "k")
        self.assertNotIn("authorization", handler.requests[0].headers)

    def test_cookie_credentials_use_one_cookie_header(self) -> None:
        handler = Recorder(httpx.Response(200, json={"status": "ck1"}))
        with sync_client(handler, api_key=None, api_keys={"api_key_cookie": "ck1"}) as api:
            api.cookies.retrieve_scenarios_cookie_auth()
        request = handler.requests[0]
        self.assertEqual(request.headers["cookie"], "auth_token=ck1")
        self.assertNotIn("authorization", request.headers)

    def test_a_call_without_credentials_raises_the_typed_auth_error(self) -> None:
        def handler(request: httpx.Request) -> httpx.Response:
            if "authorization" in request.headers or "x-api-key" in request.headers:
                return httpx.Response(200, json={"data": [], "next_cursor": None})
            return httpx.Response(401, json={"error": "unauthorized"}, headers={"x-request-id": "req_9"})

        with sync_client(handler, api_key=None) as api:
            with self.assertRaises(features.AuthenticationError) as raised:
                api.widgets.list()
        error = raised.exception
        self.assertEqual((error.status_code, error.request_id), (401, "req_9"))
        self.assertEqual(error.body, models.Error(error="unauthorized"))
        self.assertIsInstance(error, features.APIStatusError)
        self.assertIsInstance(error, features.FeaturesError)


class ErrorTest(unittest.TestCase):
    def status(self, response: httpx.Response, **options):
        """The error of `retrieve_scenario_status` answered with `response`."""
        with sync_client(Recorder(response), max_retries=0, **options) as api:
            with self.assertRaises(features.APIStatusError) as raised:
                api.errors.retrieve_scenario_status(400)
        return raised.exception

    def test_request_id_of_an_error(self) -> None:
        cases = [
            ({"x-request-id": "req_42"}, "req_42"),
            ({"request-id": "req_43"}, "req_43"),
            ({}, None),
        ]
        for headers, want in cases:
            with self.subTest(headers=headers):
                error = self.status(httpx.Response(404, json={"error": "gone"}, headers=headers))
                self.assertIsInstance(error, features.NotFoundError)
                self.assertEqual(error.request_id, want)
                self.assertEqual(str(error), 'Error code: 404 - {"error":"gone"}')
                self.assertEqual(error.headers.get("x-request-id"), headers.get("x-request-id"))

    def test_request_id_of_an_error_after_retries(self) -> None:
        response = httpx.Response(503, json={"error": "down"}, headers={"x-request-id": "req_last"})
        with sync_client(Recorder(response), max_retries=2) as api:
            with self.assertRaises(features.InternalServerError) as raised:
                api.errors.retrieve_scenario_status(400)
        self.assertEqual(raised.exception.request_id, "req_last")

    def test_the_declared_error_body_is_decoded(self) -> None:
        error = self.status(httpx.Response(409, json={"error": "status 409", "code": 409}))
        self.assertIsInstance(error, features.ConflictError)
        self.assertEqual(error.body, models.Error(error="status 409", code=409))
        self.assertEqual((error.status_code, error.response.status_code), (409, 409))

    def test_error_bodies_that_are_not_json_or_empty(self) -> None:
        cases = [
            (httpx.Response(400, text="not json at all"), features.BadRequestError, b"not json at all"),
            (
                httpx.Response(400, content=b"<html>bad</html>", headers={"content-type": "text/html"}),
                features.BadRequestError,
                b"<html>bad</html>",
            ),
            (httpx.Response(500, content=b""), features.InternalServerError, b""),
            (httpx.Response(401, content=b"{"), features.AuthenticationError, b"{"),
            (httpx.Response(403, content=b"\xff\xfe"), features.PermissionDeniedError, b"\xff\xfe"),
        ]
        for response, kind, raw in cases:
            with self.subTest(status=response.status_code, raw=raw):
                error = self.status(response)
                self.assertIsInstance(error, kind)
                self.assertIsNone(error.body)
                self.assertEqual(error.raw_body, raw)
                self.assertEqual(error.status_code, response.status_code)
                self.assertIn(str(response.status_code), str(error))
                self.assertIsInstance(error, features.FeaturesError)

    def test_error_json_of_another_shape_is_kept_as_json(self) -> None:
        for payload in ({"detail": "x"}, [1, 2], "text", 7):
            with self.subTest(payload=payload):
                error = self.status(httpx.Response(422, json=payload))
                self.assertIsInstance(error, features.UnprocessableEntityError)
                self.assertEqual(error.body, payload)

    def test_other_statuses_are_plain_status_errors(self) -> None:
        error = self.status(httpx.Response(418, json={"error": "teapot"}))
        self.assertIs(type(error), features.APIStatusError)
        self.assertEqual(error.status_code, 418)
        error = self.status(httpx.Response(302, headers={"location": "/elsewhere"}))
        self.assertEqual(error.status_code, 302)

    def test_a_2xx_body_that_does_not_decode_is_a_decode_error(self) -> None:
        bodies = [
            b'{"status": ',
            b"",
            b"   ",
            b"<html></html>",
            b'{"nope": 1}',
            b'{"status": 5}',
            b"[]",
            b"null",
            b"\xff\xfe",
        ]
        for body in bodies:
            with self.subTest(body=body):
                reply = httpx.Response(200, content=body, headers={"content-type": "application/json"})
                with sync_client(Recorder(reply), max_retries=0) as api:
                    with self.assertRaises(features.APIResponseValidationError) as raised:
                        api.content.retrieve_scenarios_malformed()
                error = raised.exception
                self.assertEqual((error.status_code, error.raw_body), (200, body))
                self.assertIsInstance(error, features.APIError)
                self.assertIsInstance(error, features.FeaturesError)

    def test_a_null_body_of_a_nullable_response_is_none(self) -> None:
        reply = httpx.Response(200, content=b"null", headers={"content-type": "application/json"})
        with sync_client(Recorder(reply)) as api:
            self.assertIsNone(api.content.retrieve_scenarios_nullable_body())

    def test_unknown_fields_of_a_response_are_tolerated(self) -> None:
        extras = {"status": "ok", "extra": 1, "nested": {"a": [1, 2, {"b": None}]}, "list": [1, "x"]}
        with sync_client(Recorder(httpx.Response(200, json=extras))) as api:
            health = api.content.list_scenarios_extra_fields()
        self.assertEqual(health.status, "ok")
        self.assertEqual(health.extra_fields["extra"], 1)
        self.assertEqual(health.to_dict(), extras)

    def test_errors_of_a_stream_are_raised_before_any_event(self) -> None:
        reply = httpx.Response(403, json={"error": "forbidden", "code": 403})
        with sync_client(Recorder(reply)) as api:
            with self.assertRaises(features.PermissionDeniedError) as raised:
                api.streaming.retrieve_scenarios_sse_error()
        self.assertEqual(raised.exception.body, models.Error(error="forbidden", code=403))


class StreamErrorTest(unittest.TestCase):
    def stream(self, body: str) -> list[object]:
        reply = httpx.Response(200, headers={"content-type": "text/event-stream"}, content=body.encode())
        with sync_client(Recorder(reply)) as api:
            return list(api.streaming.create_completion_stream(prompt="hi"))

    def test_an_error_event_is_an_api_error_with_its_message(self) -> None:
        for body in [
            'event: error\ndata: {"error": {"message": "overloaded", "type": "server_error"}}\n\n',
            'data: {"error": {"message": "overloaded"}}\n\n',
        ]:
            with self.assertRaises(features.APIStatusError) as raised:
                self.stream('data: {"delta": "a"}\n\n' + body)
            self.assertIn("overloaded", str(raised.exception))
            self.assertEqual(raised.exception.body["error"]["message"], "overloaded")

    def test_an_event_that_is_neither_the_model_nor_an_error_is_a_decode_error(self) -> None:
        with self.assertRaises(features.APIResponseValidationError):
            self.stream('data: {"unexpected": 1}\n\n')

    def test_an_error_the_model_does_not_declare_is_an_api_error(self) -> None:
        with self.assertRaises(features.APIStatusError) as raised:
            self.stream('data: {"delta": "a", "error": {"message": "overloaded"}}\n\n')
        self.assertEqual(raised.exception.body["error"]["message"], "overloaded")
        streaming = importlib.import_module(f"{features.__name__}.api._streaming")
        event = streaming.SseEvent("message", '{"error": "partial", "code": 1}')
        reply = httpx.Response(200, request=httpx.Request("GET", BASE))
        self.assertEqual(streaming._decode(event, models.Error, reply), models.Error(error="partial", code=1))

    def test_an_error_event_keeps_its_data_whatever_it_is(self) -> None:
        for data, body in [('{"error": {"message": "overloaded"}}', {"error": {"message": "overloaded"}}), ("overloaded", None)]:
            with self.subTest(data=data), self.assertRaises(features.APIStatusError) as raised:
                self.stream(f"event: error\ndata: {data}\n\n")
            self.assertIn("overloaded", str(raised.exception))
            self.assertEqual(raised.exception.body, body)
            self.assertEqual(raised.exception.raw_body, data.encode())
            self.assertEqual(raised.exception.status_code, 200)

    def test_keepalives_that_are_not_items_are_skipped(self) -> None:
        body = 'event: ping\ndata: alive\n\nevent: keepalive\ndata: {}\n\nevent: ping\ndata: {"delta": "a"}\n\ndata: {"delta": "b"}\n\n'
        self.assertEqual([chunk.delta for chunk in self.stream(body)], ["a", "b"])


class PaginationErrorTest(unittest.TestCase):
    def test_an_error_on_the_first_page_is_raised_by_the_call(self) -> None:
        handler = Recorder(httpx.Response(409, json={"error": "page_gone", "code": 409}))
        with sync_client(handler) as api:
            with self.assertRaises(features.ConflictError) as raised:
                api.errors.list_scenarios_pages()
        self.assertEqual(raised.exception.status_code, 409)
        self.assertEqual(len(handler.requests), 1)

    def test_an_error_on_a_later_page_is_not_swallowed(self) -> None:
        def handler(request: httpx.Request) -> httpx.Response:
            calls.append(request)
            if request.url.params.get("cursor") is None:
                return httpx.Response(200, json=PAGE)
            return httpx.Response(409, json={"error": "page_gone", "code": 409})

        calls: list[httpx.Request] = []
        seen: list[str] = []
        with sync_client(handler) as api:
            with self.assertRaises(features.ConflictError) as raised:
                for widget in api.errors.list_scenarios_pages():
                    seen.append(widget.id)
            self.assertEqual(seen, ["p1", "p2"])
            self.assertEqual(body_get(raised.exception.body, "error"), "page_gone")
            self.assertEqual(len(calls), 2, "the failed page is not fetched again")
            page = api.errors.list_scenarios_pages()
            self.assertEqual(page.next_cursor, "c2")
            self.assertTrue(page.has_next_page())
            with self.assertRaises(features.ConflictError):
                page.get_next_page()

    def test_an_error_on_a_later_page_of_the_async_client(self) -> None:
        async def handler(request: httpx.Request) -> httpx.Response:
            if request.url.params.get("cursor") is None:
                return httpx.Response(200, json=PAGE)
            return httpx.Response(409, json={"error": "page_gone", "code": 409})

        async def run() -> list[str]:
            seen: list[str] = []
            async with async_client(handler) as api:
                with self.assertRaises(features.ConflictError):
                    async for widget in api.errors.list_scenarios_pages():
                        seen.append(widget.id)
            return seen

        self.assertEqual(asyncio.run(run()), ["p1", "p2"])


class RetryTest(unittest.TestCase):
    def sleeps(self):
        return mock.patch.object(features.api.common.time, "sleep")

    def test_timeouts_are_retried(self) -> None:
        calls: list[httpx.Request] = []

        def handler(request: httpx.Request) -> httpx.Response:
            calls.append(request)
            if len(calls) == 1:
                raise httpx.ReadTimeout("slow", request=request)
            return httpx.Response(200, json=ITEM)

        with sync_client(handler) as api:
            self.assertEqual(api.items.retrieve("i1").id, "i1")
        self.assertEqual(len(calls), 2)
        self.assertEqual(calls[1].headers["features-retry-count"], "1")

    def test_timeouts_and_connection_errors_end_as_sdk_errors(self) -> None:
        for raised, kind in (
            (httpx.ConnectTimeout("t"), features.APITimeoutError),
            (httpx.ConnectError("c"), features.APIConnectionError),
        ):
            calls: list[httpx.Request] = []

            def handler(request: httpx.Request) -> httpx.Response:
                calls.append(request)
                raise raised

            with self.subTest(kind=kind.__name__), sync_client(handler) as api:
                with self.assertRaises(kind) as caught:
                    api.items.retrieve("i1")
                self.assertEqual(len(calls), 3, "one attempt and the 2 retries")
                self.assertIsInstance(caught.exception, features.FeaturesError)

    def test_retry_after_is_honoured_in_its_three_spellings(self) -> None:
        http_date = email.utils.formatdate(time.time() + 30, usegmt=True)
        cases = [("3", 3.0), ("2.5", 2.5), (http_date, None)]
        for header, seconds in cases:
            with self.subTest(header=header), self.sleeps() as sleep:
                handler = Recorder(unavailable(header), httpx.Response(200, json=ITEM))
                with sync_client(handler) as api:
                    api.items.retrieve("i1")
                (delay,) = [call.args[0] for call in sleep.call_args_list]
                if seconds is None:
                    self.assertTrue(25 < delay <= 30, delay)
                else:
                    self.assertEqual(delay, seconds)
        with self.sleeps() as sleep:
            handler = Recorder(
                httpx.Response(503, headers={"retry-after-ms": "250"}), httpx.Response(200, json=ITEM)
            )
            with sync_client(handler) as api:
                api.items.retrieve("i1")
            self.assertEqual([call.args[0] for call in sleep.call_args_list], [0.25])

    def test_a_retry_after_beyond_a_minute_is_not_waited_for(self) -> None:
        with self.sleeps() as sleep:
            handler = Recorder(unavailable("3600"), httpx.Response(200, json=ITEM))
            with sync_client(handler) as api:
                api.items.retrieve("i1")
            self.assertLessEqual(sleep.call_args_list[0].args[0], 8.0)

    def test_the_idempotency_key_is_the_same_on_every_attempt(self) -> None:
        handler = Recorder(unavailable(), unavailable(), httpx.Response(200, json=HEALTH))
        with sync_client(handler) as api:
            api.wire.create_charge(amount=1)
        keys = [request.headers["idempotency-key"] for request in handler.requests]
        self.assertEqual(len(keys), 3)
        self.assertTrue(keys[0].startswith("auto_"), keys)
        self.assertEqual(len(set(keys)), 1, keys)
        bodies = {request.content for request in handler.requests}
        self.assertEqual(len(bodies), 1, "the body is replayed unchanged")

    def test_a_given_idempotency_key_and_body_are_replayed(self) -> None:
        handler = Recorder(unavailable(), httpx.Response(200, json={"status": "attempts=2;key=idem-1"}))
        with sync_client(handler) as api:
            result = api.retries.scenarios_idempotent(amount=5, x_scenario_id="s1", idempotency_key="idem-1")
        self.assertEqual(result.status, "attempts=2;key=idem-1")
        self.assertEqual([r.headers["idempotency-key"] for r in handler.requests], ["idem-1", "idem-1"])
        self.assertEqual([json.loads(r.content) for r in handler.requests], [{"amount": 5}] * 2)

    def test_only_posts_get_an_automatic_idempotency_key(self) -> None:
        handler = Recorder(httpx.Response(200, json=ITEM))
        with sync_client(handler) as api:
            api.items.retrieve("i1")
        self.assertNotIn("idempotency-key", handler.requests[0].headers)

    def test_exhausted_retries_raise_the_last_error(self) -> None:
        handler = Recorder(unavailable())
        with sync_client(handler) as api:
            with self.assertRaises(features.InternalServerError) as raised:
                api.retries.retrieve_scenarios_unavailable(x_scenario_id="s")
        self.assertEqual(raised.exception.status_code, 503)
        self.assertEqual(len(handler.requests), 3)
        counts = [headers.get("features-retry-count") for headers in handler.headers]
        self.assertEqual(counts, [None, "1", "2"])

    def test_client_errors_are_not_retried(self) -> None:
        for status in (400, 401, 403, 404, 409, 422):
            with self.subTest(status=status):
                handler = Recorder(httpx.Response(status, json={"error": "x", "code": status}))
                with sync_client(handler) as api:
                    with self.assertRaises(features.APIStatusError):
                        api.errors.retrieve_scenario_status(status)
                self.assertEqual(len(handler.requests), 1)


SECRETS = ("tok", "q-secret", "h-secret", "Bearer", "authorization")
ITEM_URL = r"https://features\.test/api/v2/items/i1"


class MultipartTest(unittest.TestCase):
    def test_a_path_is_streamed_as_a_file_named_after_it_and_typed_by_the_spec_or_its_extension(
        self,
    ) -> None:
        streaming = importlib.import_module(f"{features.__name__}.api._streaming")
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "a.csv"
            path.write_bytes(b"x,y")
            for declared, sent in [
                ("image/png", "image/png"),
                ("application/octet-stream", "text/csv"),
                (None, "text/csv"),
            ]:
                with self.subTest(declared=declared):
                    [(name, (filename, content, content_type))] = streaming.multipart_files(
                        [("file", path, True, declared)]
                    )
                    self.assertEqual((name, filename, content_type), ("file", "a.csv", sent))
                    self.assertEqual(content.read(), b"x,y")
                    content.seek(0)
                    self.assertEqual(content.read(), b"x,y", "read again for a retry")
            self.assertTrue(streaming.replayable(None, streaming.multipart_files([("f", path, True, None)])))
            with self.assertRaises(FileNotFoundError):
                streaming.multipart_files([("file", Path(folder) / "missing.csv", True, None)])

    def test_a_path_upload_is_sent_again_on_a_retry(self) -> None:
        bodies: list[bytes] = []

        def handler(request: httpx.Request) -> httpx.Response:
            bodies.append(request.read())
            return unavailable() if len(bodies) == 1 else httpx.Response(200, json={"status": "ok"})

        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "notes.txt"
            path.write_bytes(b"from disk")
            with sync_client(handler) as api:
                api.streaming.upload_file(file=path, name="n")
        self.assertEqual(len(bodies), 2)
        self.assertEqual(bodies[0], bodies[1])
        self.assertIn(b"Content-Type: text/plain\r\n\r\nfrom disk", bodies[1])


class LoggingTest(unittest.TestCase):
    """The `features` logger: each attempt at DEBUG, each retry at INFO, never a credential."""

    def assert_no_secret(self, messages: list[str]) -> None:
        for message in messages:
            for secret in SECRETS:
                self.assertNotIn(secret, message)

    def retrieve(self, api):
        return api.items.retrieve(
            "i1", extra_query={"api_key": "q-secret"}, extra_headers={"x-token": "h-secret"}
        )

    def test_attempts_and_retries_are_logged(self) -> None:
        handler = Recorder(unavailable(), httpx.Response(200, json=ITEM))
        with self.assertLogs("features", "DEBUG") as logs, sync_client(handler) as api:
            self.retrieve(api)
        self.assertEqual([r.levelname for r in logs.records], ["DEBUG", "INFO", "DEBUG"])
        messages = [r.getMessage() for r in logs.records]
        self.assertRegex(messages[0], rf"^GET {ITEM_URL} -> 503 in \d+\.\d{{3}}s \(retry 0\)$")
        self.assertRegex(messages[1], rf"^Retrying GET {ITEM_URL} in 0\.00s \(retry 1 of 2\)$")
        self.assertRegex(messages[2], rf"^GET {ITEM_URL} -> 200 in \d+\.\d{{3}}s \(retry 1\)$")
        self.assert_no_secret(messages)

    def test_info_logs_the_retries_only(self) -> None:
        handler = Recorder(unavailable(), httpx.Response(200, json=ITEM))
        with self.assertLogs("features", "INFO") as logs, sync_client(handler) as api:
            self.retrieve(api)
        self.assertEqual([r.levelname for r in logs.records], ["INFO"])

    def test_async_attempts_and_connection_errors_are_logged(self) -> None:
        def handler(request: httpx.Request) -> httpx.Response:
            raise httpx.ConnectError("refused by h-secret")

        async def run() -> None:
            async with async_client(handler, max_retries=1) as api:
                with self.assertRaises(features.APIConnectionError):
                    await self.retrieve(api)

        with self.assertLogs("features", "DEBUG") as logs:
            asyncio.run(run())
        messages = [r.getMessage() for r in logs.records]
        self.assertEqual(len(messages), 3, messages)
        self.assertRegex(messages[0], rf"^GET {ITEM_URL} -> ConnectError in ")
        self.assertRegex(messages[1], rf"^Retrying GET {ITEM_URL} in ")
        self.assert_no_secret(messages)

    def test_the_log_variable_configures_the_logger(self) -> None:
        logger = logging.getLogger("features")
        self.addCleanup(setattr, logger, "handlers", [])
        self.addCleanup(setattr, logger, "propagate", True)
        self.addCleanup(logger.setLevel, logging.NOTSET)
        cases = [
            ("debug", logging.DEBUG, 1),
            ("INFO", logging.INFO, 1),
            ("verbose", logging.NOTSET, 0),
            (None, logging.NOTSET, 0),
        ]
        for value, level, handlers in cases:
            with self.subTest(value=value), mock.patch.object(logging.root, "handlers", []):
                logger.handlers = []
                logger.setLevel(logging.NOTSET)
                logger.propagate = True
                with mock.patch.dict(os.environ, {"FEATURES_LOG": value or ""}):
                    features.api.common._setup_logging()
                self.assertEqual((logger.level, len(logger.handlers)), (level, handlers))
                # A root handler, as `logging.basicConfig` adds, would print each line again.
                self.assertEqual(logger.propagate, handlers == 0)

    def test_the_log_variable_keeps_a_configured_logger(self) -> None:
        logger = logging.getLogger("features")
        self.addCleanup(logger.setLevel, logging.NOTSET)
        logger.setLevel(logging.WARNING)
        with mock.patch.dict(os.environ, {"FEATURES_LOG": "debug"}):
            features.api.common._setup_logging()
        self.assertEqual((logger.level, logger.handlers), (logging.WARNING, []))


class MiddlewareTest(unittest.TestCase):
    def test_middleware_runs_inside_the_retry_loop_in_order(self) -> None:
        log: list[str] = []

        def named(name: str):
            def middleware(request: httpx.Request, call_next):
                log.append(f"{name}>{request.headers.get('features-retry-count', '0')}")
                response = call_next(request)
                log.append(f"{name}<{response.status_code}")
                return response

            return middleware

        handler = Recorder(unavailable(), httpx.Response(200, json=ITEM))
        with sync_client(handler, middleware=[named("a"), named("b")]) as api:
            api.items.retrieve("i1")
        self.assertEqual(log, ["a>0", "b>0", "b<503", "a<503", "a>1", "b>1", "b<200", "a<200"])
        self.assertEqual(len(handler.requests), 2)

    def test_middleware_sees_the_final_request_and_can_answer_itself(self) -> None:
        seen: list[httpx.Request] = []
        answers = [unavailable(), httpx.Response(200, json=HEALTH)]

        def middleware(request: httpx.Request, call_next):
            seen.append(request)
            return answers.pop(0)

        with sync_client(Recorder(httpx.Response(500)), middleware=[middleware]) as api:
            api.wire.create_charge(amount=1)
        self.assertEqual(len(seen), 2, "a response of the middleware is retried like any other")
        self.assertEqual(seen[0].headers["authorization"], "Bearer tok")
        self.assertTrue(seen[0].headers["idempotency-key"].startswith("auto_"))
        self.assertEqual(seen[0].headers["idempotency-key"], seen[1].headers["idempotency-key"])

    def test_async_middleware_runs_inside_the_retry_loop(self) -> None:
        log: list[str] = []
        queue = [unavailable(), httpx.Response(200, json=ITEM)]

        async def handler(request: httpx.Request) -> httpx.Response:
            return queue.pop(0)

        async def middleware(request: httpx.Request, call_next):
            log.append("before")
            response = await call_next(request)
            log.append(f"after {response.status_code}")
            return response

        async def run() -> object:
            async with async_client(handler, middleware=[middleware]) as api:
                return await api.items.retrieve("i1")

        self.assertEqual(asyncio.run(run()).id, "i1")
        self.assertEqual(log, ["before", "after 503", "before", "after 200"])


class Chunks:
    """A response body sent chunk by chunk, counting what the server produced; it may fail last."""

    def __init__(self, *chunks: bytes, fail: Exception | None = None) -> None:
        self.chunks = chunks
        self.fail = fail
        self.sent = 0

    def __iter__(self):
        for chunk in self.chunks:
            self.sent += 1
            yield chunk
        if self.fail is not None:
            raise self.fail

    async def __aiter__(self):
        for chunk in self:
            yield chunk


class BinaryResponseTest(unittest.TestCase):
    def test_chunks_are_read_as_they_are_consumed(self) -> None:
        body = Chunks(b"ab", b"cd", b"ef")
        with sync_client(Recorder(httpx.Response(200, content=iter(body)))) as api:
            with api.content.download_blob() as download:
                self.assertEqual(body.sent, 0, "the body is not read before it is consumed")
                self.assertEqual(download.status_code, 200)
                chunks = download.iter_bytes()
                self.assertEqual(next(chunks), b"ab")
                self.assertEqual(body.sent, 1)
                self.assertEqual(list(chunks), [b"cd", b"ef"])
            self.assertTrue(download.response.is_closed)

    def test_read_gives_the_whole_body_once_and_releases_the_connection(self) -> None:
        handler = Recorder(httpx.Response(200, content=iter(Chunks(b"ab", b"cd")), headers={"content-type": "image/png"}))
        with sync_client(handler) as api:
            download = api.content.download_image()
            self.assertEqual(download.content_type, "image/png")
            self.assertEqual(download.read(), b"abcd")
            self.assertTrue(download.response.is_closed)
            self.assertEqual(download.read(), b"abcd")
            self.assertEqual(list(download.iter_bytes(3)), [b"abc", b"d"])

    def test_write_to_file_streams_the_body_to_disk(self) -> None:
        handler = Recorder(httpx.Response(200, content=iter(Chunks(*(bytes([i]) * 1000 for i in range(5))))))
        with tempfile.TemporaryDirectory() as directory, sync_client(handler) as api:
            path = Path(directory) / "blob.bin"
            download = api.content.download_blob()
            download.write_to_file(path)
            self.assertTrue(download.response.is_closed)
            self.assertEqual(path.read_bytes(), b"".join(bytes([i]) * 1000 for i in range(5)))

    def test_an_error_status_is_raised_before_any_body(self) -> None:
        handler = Recorder(httpx.Response(404, json={"error": "gone"}))
        with sync_client(handler, max_retries=0) as api:
            with self.assertRaises(features.NotFoundError) as raised:
                api.content.download_blob()
        self.assertEqual(body_get(raised.exception.body, "error"), "gone")

    def test_retries_stop_once_the_headers_arrive(self) -> None:
        body = Chunks(b"ab", fail=httpx.ReadError("reset"))
        handler = Recorder(unavailable(), httpx.Response(200, content=iter(body)))
        with sync_client(handler) as api:
            download = api.content.download_blob()
            self.assertEqual(len(handler.requests), 2, "a 503 before the body is retried")
            with self.assertRaises(features.APIConnectionError):
                download.read()
        self.assertEqual(len(handler.requests), 2, "a failure while reading the body is not")

    def test_a_read_timeout_of_the_body_is_a_timeout_error(self) -> None:
        body = Chunks(b"ab", fail=httpx.ReadTimeout("slow"))
        with sync_client(Recorder(httpx.Response(200, content=iter(body)))) as api:
            with self.assertRaises(features.APITimeoutError):
                list(api.content.download_blob())

    def test_a_response_dropped_unread_releases_its_connection(self) -> None:
        with sync_client(Recorder(httpx.Response(200, content=iter(Chunks(b"ab"))))) as api:
            download = api.content.download_blob()
            response = download.response
            del download
            self.assertTrue(response.is_closed)

    def test_the_raw_response_holds_the_unread_body(self) -> None:
        handler = Recorder(httpx.Response(200, content=b"abc", headers={"x-request-id": "req_1"}))
        with sync_client(handler) as api:
            raw = api.with_raw_response.content.download_blob()
            self.assertEqual(raw.request_id, "req_1")
            with raw.parse() as download:
                self.assertEqual(download.read(), b"abc")

    def test_the_async_client_streams_the_body(self) -> None:
        body = Chunks(b"ab", b"cd")

        def handler(request: httpx.Request) -> httpx.Response:
            return httpx.Response(200, content=body.__aiter__())

        async def run() -> None:
            async with async_client(handler) as api:
                async with await api.content.download_blob() as download:
                    self.assertEqual(body.sent, 0)
                    chunks = []
                    async for chunk in download:
                        chunks.append(chunk)
                        self.assertEqual(body.sent, len(chunks))
                    self.assertEqual(chunks, [b"ab", b"cd"])
                self.assertTrue(download.response.is_closed)
                self.assertEqual(await (await api.content.download_blob()).read(), b"abcd")
                with tempfile.TemporaryDirectory() as directory:
                    path = Path(directory) / "blob.bin"
                    await (await api.content.download_blob()).write_to_file(path)
                    self.assertEqual(path.read_bytes(), b"abcd")

        asyncio.run(run())


class CancellationTest(unittest.TestCase):
    def test_a_cancelled_call_is_not_retried_and_the_client_stays_usable(self) -> None:
        calls: list[httpx.Request] = []
        async def handler(request: httpx.Request) -> httpx.Response:
            calls.append(request)
            if len(calls) <= 2:
                await asyncio.sleep(30)
            return httpx.Response(200, json=ITEM)

        async def run() -> None:
            async with async_client(handler) as api:
                task = asyncio.ensure_future(api.items.retrieve("i1"))
                while not calls:
                    await asyncio.sleep(0.005)
                task.cancel()
                with self.assertRaises(asyncio.CancelledError):
                    await task
                with self.assertRaises(asyncio.TimeoutError):
                    await asyncio.wait_for(api.items.retrieve("i1"), 0.05)
                self.assertEqual(len(calls), 2, "neither cancelled call was retried")
                self.assertEqual((await api.items.retrieve("i1")).id, "i1")

        started = time.monotonic()
        asyncio.run(run())
        self.assertLess(time.monotonic() - started, 10)

    def test_a_cancelled_retry_wait_stops_the_call(self) -> None:
        calls: list[httpx.Request] = []

        async def handler(request: httpx.Request) -> httpx.Response:
            calls.append(request)
            return unavailable("30")

        async def run() -> None:
            async with async_client(handler) as api:
                with self.assertRaises(asyncio.TimeoutError):
                    await asyncio.wait_for(api.items.retrieve("i1"), 0.1)
                self.assertEqual(len(calls), 1)

        asyncio.run(run())


if __name__ == "__main__":
    unittest.main()

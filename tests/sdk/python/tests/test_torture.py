"""Generates tests/fixtures/torture.yaml, then round-trips its models and drives the client."""

import asyncio
import importlib
import inspect
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
from datetime import datetime, timezone
from pathlib import Path

import httpx

WORK = Path(tempfile.mkdtemp(prefix="perseid-torture-"))


def generate(
    name: str, python: str = "", spec: str | None = None, base_url: str | None = "https://torture.test/v1"
) -> object:
    """Generates the fixture, or `spec`, as the `name` package and imports it."""
    fixtures = os.environ.get("FIXTURES")
    assert fixtures, "set FIXTURES to the perseid tests/fixtures directory"
    project = WORK / name
    project.mkdir()
    if spec is None:
        shutil.copy(Path(fixtures) / "torture.yaml", project / "openapi.yaml")
    else:
        (project / "openapi.yaml").write_text(spec)
    base = f'base_url = "{base_url}"\n' if base_url else ""
    (project / "perseid.toml").write_text(
        f'spec = "openapi.yaml"\nname = "{name.title()}"\nsdks = ["python"]\nidempotency_keys = true\n{base}[python]\n{python}'
    )
    subprocess.run(["perseid", "generate"], cwd=project, check=True, capture_output=True)
    sys.path.insert(0, str(project / "python"))
    importlib.import_module(f"{name}.models")
    return importlib.import_module(name)


def tearDownModule() -> None:
    shutil.rmtree(WORK, ignore_errors=True)


ADJACENT_SPEC = """
openapi: 3.1.0
info: {title: Adjacent, version: "1"}
paths:
  /sources:
    get:
      operationId: get_source
      responses:
        "200":
          description: ok
          content: {application/json: {schema: {$ref: '#/components/schemas/Source'}}}
components:
  schemas:
    Config: {type: object, required: [url], properties: {url: {type: string}}}
    Source:
      type: object
      required: [id]
      properties:
        id: {type: string}
        note: {type: [string, "null"]}
      oneOf:
        - type: object
          required: [type, config]
          properties:
            type: {type: string, enum: [http]}
            config: {$ref: '#/components/schemas/Config'}
        - type: object
          required: [type]
          properties:
            type: {type: string, enum: [none]}
"""

EXPANDABLE_SPEC = """
openapi: 3.1.0
info: {title: Expandable, version: "1"}
paths:
  /charges/{id}:
    get:
      operationId: GetChargesId
      parameters: [{name: id, in: path, required: true, schema: {type: string}}]
      responses:
        "200":
          description: ok
          content: {application/json: {schema: {$ref: '#/components/schemas/Charge'}}}
components:
  schemas:
    Customer: {type: object, required: [id], properties: {id: {type: string}}}
    Shipping: {type: object, properties: {name: {type: string}}}
    Charge:
      type: object
      required: [customer]
      properties:
        customer: {anyOf: [{type: string}, {$ref: '#/components/schemas/Customer'}]}
        shipping: {anyOf: [{$ref: '#/components/schemas/Shipping'}, {type: string, enum: [""]}]}
        tags: {anyOf: [{type: array, items: {type: string}}, {type: boolean}]}
"""

PAGED_SPEC = """
openapi: 3.1.0
info: {title: Paged, version: "1"}
paths:
  /items:
    get:
      operationId: list_items
      x-pagination: {page: page, items: result.items, total_pages: result.meta.total_pages}
      parameters: [{name: page, in: query, schema: {type: integer}}]
      responses:
        "200":
          description: ok
          content: {application/json: {schema: {$ref: '#/components/schemas/ItemPage'}}}
  /logs:
    get:
      operationId: list_logs
      x-pagination: {cursor: after, next_cursor: next, has_more: has_more}
      parameters: [{name: after, in: query, schema: {type: string}}]
      responses:
        "200":
          description: ok
          content: {application/json: {schema: {$ref: '#/components/schemas/LogPage'}}}
  /chat:
    post:
      operationId: create_chat
      requestBody:
        required: false
        content: {application/json: {schema: {$ref: '#/components/schemas/ChatRequest'}}}
      responses:
        "200":
          description: the answer, or its chunks with `stream`
          content:
            application/json: {schema: {$ref: '#/components/schemas/Item'}}
            text/event-stream: {schema: {$ref: '#/components/schemas/Item'}}
components:
  schemas:
    ChatRequest: {type: object, properties: {stream: {type: boolean}}}
    Item: {type: object, required: [id], properties: {id: {type: string}}}
    ItemPage:
      type: object
      properties:
        result:
          type: object
          properties:
            items: {type: array, items: {$ref: '#/components/schemas/Item'}}
            meta: {type: object, properties: {total_pages: {type: integer}}}
    LogPage:
      type: object
      required: [data]
      properties:
        data: {type: array, items: {$ref: '#/components/schemas/Item'}}
        next: {type: [string, "null"]}
        has_more: {type: boolean}
"""

torture = generate("torture")
paged = generate("paged", spec=PAGED_SPEC)
expandable = generate("expandable", spec=EXPANDABLE_SPEC)
adjacent = generate("adjacent", spec=ADJACENT_SPEC, base_url=None)
from torture import RateLimitError, Torture, models  # noqa: E402
from torture import (  # noqa: E402
    APIConnectionError,
    APIStatusError,
    APITimeoutError,
    InternalServerError,
    NotFoundError,
    TortureError,
    UnprocessableEntityError,
)
from torture.serialization import UNSET, UnknownVariant, from_json_value, to_json_value  # noqa: E402
from torture.unions import as_variant, expandable_id  # noqa: E402

SAMPLES = {
    "Thing": {
        "id": "t1",
        "name": "n",
        "created_at": "2024-01-02T03:04:05.123456+00:00",
        "kind": "beta-2",
        "count": 9007199254740993,
        "nullable_required": None,
        "nullable_optional": None,
        "nullable_ref": None,
        "tags": ["a"],
        "metadata": {"k": "v"},
        "attrs": {"x": [1, 2]},
        "amount": "12.50",
        "birthday": "2024-02-29",
        "priority": 10,
        "nested_map": {"a": [{"line1": "l"}]},
        "anyof_nullable_ref": {"line1": "x"},
    },
    "Shape": {"type": "circle", "radius": 1.5},
    "Pet": {"pet_type": "Cat", "meow": True},
    "Composed": {
        "id": "b1",
        "created_at": "2024-01-02T03:04:05+00:00",
        "extra": "e",
        "sibling_prop": "s",
    },
    "TreeNode": {
        "value": "root",
        "children": [{"value": "c", "children": []}],
        "parent": None,
        "next": {"value": "n", "children": []},
    },
    "UnionHolder": {
        "shape": {"type": "circle", "radius": 1.0},
        "shapes": [{"type": "square", "side": 1.0}, {"type": "circle", "radius": 2.0}],
        "maybe_shape": None,
        "shape_map": {"k": {"type": "square", "side": 3.0}},
        "inline_union": ["a", "b"],
        "empty": {},
        "free_form": {"any": 1},
        "counts": {"a": 9007199254740993},
    },
    "ThingPatch": {"description": None, "count": None},
    "Activity": {"kind": "reopened", "at": "2024-01-02T03:04:05+00:00"},
    "Reserved": {
        "class": "c",
        "type": "t",
        "self": "s",
        "1leading": "1",
        "with space": "w",
        "properties": {"k": "v"},
        "extra": "x",
        "extra_fields": "e",
        "additional_properties": "a",
        "any_properties": "p",
    },
    "Widget": {"id": "w", "name": "n", "reactions": {"+1": 1, "-1": 2}},
}


def round_trip(package: object, model: str, data: object) -> object:
    annotation = getattr(package.models, model)
    parsed = package.serialization.from_json_value(annotation, data)
    return json.loads(json.dumps(package.serialization.to_json_value(parsed, annotation)))


class ModelTest(unittest.TestCase):
    def test_models_round_trip(self) -> None:
        for model, data in SAMPLES.items():
            with self.subTest(model=model):
                self.assertEqual(round_trip(torture, model, data), data)

    def test_unknown_enum_values_are_kept(self) -> None:
        kind = models.Kind("brand-new")
        self.assertEqual(kind, "brand-new")
        self.assertFalse(kind.is_known)
        self.assertTrue(models.Kind.ALPHA.is_known)
        self.assertIs(models.Kind("alpha"), models.Kind.ALPHA)
        self.assertNotIn("brand-new", list(models.Kind))
        priority = models.Priority(99)
        self.assertEqual((priority, priority.is_known), (99, False))
        data = {**SAMPLES["Thing"], "kind": "brand-new", "priority": 99}
        self.assertEqual(round_trip(torture, "Thing", data), data)

    def test_unknown_union_variants_are_kept(self) -> None:
        data = {"type": "triangle", "a": 1}
        self.assertEqual(from_json_value(models.Shape, data), UnknownVariant("triangle", data))
        self.assertEqual(round_trip(torture, "Shape", data), data)
        data = {"kind": "archived", "by": "b"}
        activity = models.Activity.from_dict(data)
        self.assertEqual(activity.content, UnknownVariant("archived", data))
        self.assertEqual(activity.to_dict(), data)

    def test_unions_are_the_variant_models(self) -> None:
        shape = from_json_value(models.Shape, SAMPLES["Shape"])
        self.assertIsInstance(shape, models.Circle)
        square = models.Square(side=2, type="ignored")
        self.assertEqual(to_json_value(square, models.Shape), {"side": 2, "type": "square"})
        pet = from_json_value(models.Pet, {"pet_type": "Dog", "bark": "woof"})
        self.assertIsInstance(pet, models.Dog)
        self.assertEqual(pet.extra_fields, {}, "the tag is not an unknown property")
        self.assertEqual(to_json_value(pet, models.Pet), {"pet_type": "Dog", "bark": "woof"})

    def test_adjacently_tagged_unions_with_shared_fields(self) -> None:
        Source = adjacent.models.Source
        for data in (
            {"id": "1", "type": "http", "config": {"url": "u"}},
            {"id": "2", "type": "none", "note": None},
            {"id": "3", "type": "ftp", "config": {"x": 1}},
        ):
            with self.subTest(data=data):
                self.assertEqual(Source.from_dict(data).to_dict(), data)
        source = Source(id="1", type="http", config=adjacent.models.Config(url="u"))
        self.assertIsNone(source.note, "responses read an absent nullable field as None")

    def test_optional_nullable_fields_of_responses_are_none_when_absent(self) -> None:
        data = {key: value for key, value in SAMPLES["Thing"].items() if key != "nullable_optional"}
        self.assertIsNone(models.Thing.from_dict(data).nullable_optional)
        self.assertIsNone(models.Thing.from_dict(SAMPLES["Thing"]).nullable_optional)

    def test_optional_nullable_fields_tell_unset_from_null(self) -> None:
        self.assertEqual(models.ThingPatch().to_dict(), {})
        self.assertEqual(models.ThingPatch(description=None).to_dict(), {"description": None})
        self.assertIs(models.ThingPatch.from_dict({}).description, UNSET)
        self.assertIsNone(models.ThingPatch.from_dict({"description": None}).description)
        self.assertFalse(UNSET)

    def test_required_nullable_fields_are_always_sent(self) -> None:
        thing = models.Thing.from_dict(SAMPLES["Thing"])
        self.assertIsNone(thing.nullable_required)
        self.assertIn("nullable_required", thing.to_dict())
        self.assertNotIn("email", thing.to_dict())

    def test_models_are_keyword_only(self) -> None:
        with self.assertRaises(TypeError):
            models.Address("line")  # type: ignore[misc]

    def test_unions_of_objects_pick_their_variant_and_keep_unknown_shapes(self) -> None:
        cases = [
            ("a0", str, "a0"),
            ({"id": "a1", "object": "account", "email": "e"}, models.Account, "a1"),
            ({"deleted": True, "id": "a2", "object": "account"}, models.DeletedAccount, "a2"),
            ({"object": "account_v2", "id": "a3"}, UnknownVariant, None),
        ]
        for payload, kind, id in cases:
            unions = models.ObjectUnions.from_dict({"account": payload})
            self.assertIsInstance(unions.account, kind)
            self.assertEqual(expandable_id(unions.account), id)
            self.assertEqual(unions.to_dict(), {"account": payload})
        deleted = models.ObjectUnions.from_dict({"account": cases[2][0]}).account
        self.assertEqual(as_variant(deleted, models.Account).id, "a2")

        payload = {
            "source": {"file_id": "f"},
            "sources": [{"url": "u", "detail": "d"}, {"path": "p"}],
            "document": {"title": "t", "author": "a"},
            "loose": {"title": "t"},
        }
        unions = models.ObjectUnions.from_dict(payload)
        self.assertIsInstance(unions.source, models.FileSource)
        self.assertIsInstance(unions.sources[0], models.UrlSource)
        self.assertEqual(unions.sources[1], UnknownVariant("", {"path": "p"}))
        self.assertIsInstance(unions.document, models.Article)
        # Best match is the default for unions no property tells apart, even without
        # `x-perseid-union`: the tie goes to the first variant.
        self.assertEqual(unions.loose, models.Draft(title="t"))
        self.assertEqual(unions.to_dict(), payload)
        draft = models.ObjectUnions.from_dict({"document": {"title": "t"}}).document
        self.assertIsInstance(draft, models.Draft, "ties go to the first variant")
        untitled = models.ObjectUnions.from_dict({"document": {"body": "b"}}).document
        self.assertIsInstance(untitled, UnknownVariant)

    def test_unknown_properties_stay_with_the_model_that_owns_them(self) -> None:
        composed = models.Composed.from_dict({**SAMPLES["Composed"], "new": 1})
        self.assertEqual((composed.extra_fields, composed.base.extra_fields), ({"new": 1}, {}))
        composed.extra = "changed"
        self.assertEqual(composed.to_dict()["extra"], "changed")
        data = {"type": "circle", "radius": 1.0, "color": "red"}
        shape = from_json_value(models.Shape, data)
        self.assertEqual(shape.extra_fields, {"color": "red"})
        self.assertEqual(to_json_value(shape, models.Shape), data)
        data = {"kind": "closed", "by": "b", "color": "red"}
        activity = models.Activity.from_dict(data)
        self.assertEqual((activity.extra_fields, activity.content.extra_fields), ({}, {"color": "red"}))
        self.assertEqual(activity.to_dict(), data)
        source = adjacent.models.Source.from_dict({"id": "1", "type": "none", "v": 2})
        self.assertEqual(source.extra_fields, {"v": 2})
        self.assertEqual(source.to_dict(), {"id": "1", "type": "none", "v": 2})

    def test_recursive_models_import_and_parse(self) -> None:
        tree = models.TreeNode.from_dict(SAMPLES["TreeNode"])
        self.assertEqual(tree.children[0].value, "c")
        self.assertIsInstance(tree.next, models.TreeNode)


# Retries without backoff, to keep tests fast.
torture.api.common.INITIAL_RETRY_DELAY = 0.0


def client(handler, **options) -> Torture:
    """A client of `handler`."""
    transport = httpx.MockTransport(handler)
    return Torture(api_key="token", timeout=5, http_client=httpx.Client(transport=transport), **options)


THING = {**SAMPLES["Thing"], "id": "a/b"}


class ClientTest(unittest.TestCase):
    def setUp(self) -> None:
        self.requests: list[httpx.Request] = []

    def respond(self, *responses: httpx.Response):
        queue = list(responses)

        def handler(request: httpx.Request) -> httpx.Response:
            self.requests.append(request)
            return queue.pop(0) if len(queue) > 1 else queue[0]

        return handler

    def test_request_shape(self) -> None:
        with client(self.respond(httpx.Response(200, json=THING))) as api:
            thing = api.things.retrieve("a/b", extra_headers={"X-Extra": "1"}, timeout=2)
        request = self.requests[0]
        self.assertEqual(thing.id, "a/b")
        self.assertEqual(request.url.raw_path, b"/v1/things/a%2Fb")
        self.assertEqual(request.headers["x-extra"], "1")
        self.assertEqual(request.headers["authorization"], "Bearer token")
        self.assertEqual(request.extensions["timeout"]["read"], 2)

    def test_headers_of_the_call_or_client_win_over_its_credentials(self) -> None:
        with client(self.respond(httpx.Response(200, json=THING))) as api:
            api.things.retrieve("t", extra_headers={"Authorization": "Bearer call"})
            api.with_options(default_headers={"authorization": "Bearer client"}).things.retrieve("t")
            api.things.retrieve("t")
        auth = [r.headers.get_list("authorization") for r in self.requests]
        self.assertEqual(auth, [["Bearer call"], ["Bearer client"], ["Bearer token"]])

    def test_a_client_without_base_url_names_both_ways_to_give_one(self) -> None:
        with self.assertRaises(adjacent.AdjacentError) as raised:
            adjacent.Adjacent(api_key="k")
        self.assertIn("base_url", str(raised.exception))
        self.assertIn("ADJACENT_BASE_URL", str(raised.exception))
        os.environ["ADJACENT_BASE_URL"] = "https://adjacent.test"
        try:
            self.assertEqual(adjacent.Adjacent(api_key="k")._cfg.base_path, "https://adjacent.test")
        finally:
            del os.environ["ADJACENT_BASE_URL"]
        with self.assertRaises(TypeError):
            Torture("token")  # type: ignore[misc]

    def test_query_and_header_params(self) -> None:
        with client(self.respond(httpx.Response(200, json={"data": [], "total": 0}))) as api:
            api.things.list(
                x_required="r",
                ids=["a", "b"],
                csv_ids=["c", "d"],
                kind=models.Kind.BETA_2,
                since=datetime(2024, 1, 2, tzinfo=timezone.utc),
                flag=False,
            )
        request = self.requests[0]
        params = request.url.params
        self.assertEqual(params.get_list("ids"), ["a", "b"])
        self.assertEqual(params["csv_ids"], "c,d")
        self.assertEqual(params["kind"], "beta-2")
        self.assertEqual(params["flag"], "false")
        self.assertEqual(params["since"], "2024-01-02T00:00:00+00:00")
        self.assertEqual(request.headers["x-required"], "r")
        self.assertNotIn("x-trace-id", request.headers)

    def test_extra_query_replaces_a_param_of_the_method(self) -> None:
        with client(self.respond(httpx.Response(200, json={"data": [], "total": 0}))) as api:
            api.things.list(x_required="r", flag=False, extra_query={"flag": True})
        self.assertEqual(self.requests[0].url.params.get_list("flag"), ["true"])

    def test_list_bodies_and_odd_query_names(self) -> None:
        widget = {"id": "w", "name": "n"}
        with client(self.respond(httpx.Response(200, json=[widget]))) as api:
            created = api.widgets.bulk([models.WidgetUpdate(name="n")])
            api.widgets.list(date_created_lt="2024-01-01")
        self.assertEqual(created, [models.Widget(id="w", name="n")])
        self.assertEqual(json.loads(self.requests[0].content), [{"name": "n"}])
        self.assertEqual(self.requests[1].url.params["DateCreated<"], "2024-01-01")

    def test_object_bodies_are_keyword_arguments(self) -> None:
        with client(self.respond(httpx.Response(200, json=THING))) as api:
            api.things.create(name="n", kind="beta-2", priority=10)
            api.things.create(name="n", kind=models.Kind.ALPHA, metadata={"k": "v"})
        bodies = [json.loads(r.content) for r in self.requests]
        self.assertEqual(bodies[0], {"name": "n", "kind": "beta-2", "priority": 10})
        self.assertEqual(bodies[1], {"name": "n", "kind": "alpha", "metadata": {"k": "v"}})

    def test_extra_query_body_and_headers_of_a_call(self) -> None:
        with client(self.respond(httpx.Response(200, json=THING))) as api:
            api.things.create(
                name="n",
                kind="alpha",
                extra_query={"debug": True, "tags": ["a", "b"]},
                extra_body={"beta": {"x": 1}, "name": "over"},
                extra_headers={"x-extra": "1"},
            )
            api.things.retrieve("t", extra_body={"only": 1})
        first, second = self.requests
        self.assertEqual(first.url.params.multi_items(), [("debug", "true"), ("tags", "a"), ("tags", "b")])
        self.assertEqual(json.loads(first.content), {"name": "over", "kind": "alpha", "beta": {"x": 1}})
        self.assertEqual(first.headers["x-extra"], "1")
        self.assertEqual(json.loads(second.content), {"only": 1})

    def test_a_user_idempotency_key_replaces_the_automatic_one(self) -> None:
        with client(self.respond(httpx.Response(200, json=THING))) as api:
            api.things.create(name="n", kind="alpha")
            api.things.create(name="n", kind="alpha", idempotency_key="mine")
            api.things.create(name="n", kind="alpha", extra_headers={"IDEMPOTENCY-KEY": "extra"})
        keys = [r.headers.get_list("idempotency-key") for r in self.requests]
        self.assertTrue(keys[0][0].startswith("auto_"))
        self.assertEqual(keys[1:], [["mine"], ["extra"]])

    def test_patch_sends_explicit_nulls(self) -> None:
        with client(self.respond(httpx.Response(200, json=THING))) as api:
            api.things.update("t", name="n", description=None)
            api.things.update("t", name="n")
        self.assertEqual(json.loads(self.requests[0].content), {"name": "n", "description": None})
        self.assertEqual(json.loads(self.requests[1].content), {"name": "n"})

    def test_retries_wait_the_delay_the_server_asks_for(self) -> None:
        for status in (429, 500, 408):
            with self.subTest(status=status):
                self.requests.clear()
                responses = (
                    httpx.Response(status, headers={"retry-after-ms": "200"}),
                    httpx.Response(200, json=THING),
                )
                with client(self.respond(*responses), max_retries=1) as api:
                    started = time.monotonic()
                    api.things.retrieve("t")
                self.assertGreaterEqual(time.monotonic() - started, 0.2)
                self.assertEqual(self.requests[1].headers["torture-retry-count"], "1")

    def test_max_retries_can_change_for_one_call(self) -> None:
        responses = (httpx.Response(503), httpx.Response(503), httpx.Response(200, json=THING))
        with client(self.respond(*responses), max_retries=1) as api:
            with self.assertRaises(InternalServerError):
                api.with_options(max_retries=0).things.retrieve("t")
            with self.assertRaises(InternalServerError):
                api.things.retrieve("t", max_retries=0)
            self.assertEqual(len(self.requests), 2)
            self.assertEqual(api.things.retrieve("t", max_retries=1).id, "a/b")

    def test_non_idempotent_requests_are_not_replayed_on_429(self) -> None:
        responses = (httpx.Response(429, headers={"retry-after": "0"}), httpx.Response(200))
        with client(self.respond(*responses), max_retries=1) as api:
            with self.assertRaises(RateLimitError):
                api.things.update("t")
        self.assertEqual(len(self.requests), 1)

    def test_non_idempotent_requests_are_not_replayed_on_5xx(self) -> None:
        responses = (
            httpx.Response(500, headers={"x-request-id": "req_1"}),
            httpx.Response(500),
            httpx.Response(200, json=THING),
        )
        with client(self.respond(*responses), max_retries=1) as api:
            with self.assertRaises(APIStatusError) as raised:
                api.things.update("t")
            self.assertEqual(raised.exception.headers["x-request-id"], "req_1")
            self.assertEqual(len(self.requests), 1)
            api.things.create(name="n", kind="alpha")
        self.assertEqual(len(self.requests), 3)
        self.assertEqual(self.requests[1].headers["idempotency-key"],
                         self.requests[2].headers["idempotency-key"])

    def test_timeouts_are_retried_then_raised(self) -> None:
        def handler(request: httpx.Request) -> httpx.Response:
            self.requests.append(request)
            raise httpx.ReadTimeout("slow", request=request)

        with client(handler, max_retries=2) as api:
            with self.assertRaises(APITimeoutError) as raised:
                api.things.retrieve("t")
        self.assertEqual(len(self.requests), 3)
        self.assertIs(raised.exception.request, self.requests[-1])

    def test_connection_errors_are_sdk_errors(self) -> None:
        def handler(request: httpx.Request) -> httpx.Response:
            raise httpx.ConnectError("refused", request=request)

        with client(handler, max_retries=0) as api:
            with self.assertRaises(APIConnectionError) as raised:
                api.things.retrieve("t")
        self.assertNotIsInstance(raised.exception, APITimeoutError)
        self.assertIsInstance(raised.exception, TortureError)
        self.assertIsInstance(raised.exception.__cause__, httpx.ConnectError)

    def test_raw_responses_carry_status_headers_and_the_decoded_body(self) -> None:
        response = httpx.Response(200, json=THING, headers={"x-request-id": "req_1"})
        with client(self.respond(response)) as api:
            raw = api.with_raw_response.things.retrieve("a/b")
            same = api.things.with_raw_response.retrieve("a/b")
        self.assertEqual((raw.status_code, raw.request_id, raw.method), (200, "req_1", "GET"))
        self.assertEqual(raw.parse(), same.parse())
        self.assertEqual(raw.parse().id, "a/b")

    def test_unknown_properties_are_kept_and_sent_back(self) -> None:
        data = {**THING, "added_later": {"x": 1}}
        with client(self.respond(httpx.Response(200, json=data))) as api:
            thing = api.things.retrieve("a/b")
        self.assertEqual(thing.extra_fields, {"added_later": {"x": 1}})
        self.assertEqual(thing.added_later, {"x": 1})  # type: ignore[attr-defined]
        self.assertEqual(json.loads(json.dumps(thing.to_dict())), data)
        with self.assertRaises(AttributeError):
            thing.missing  # type: ignore[attr-defined]  # noqa: B018

    def test_async_client(self) -> None:
        async def handler(request: httpx.Request) -> httpx.Response:
            return httpx.Response(200, json=SAMPLES["Shape"])

        async def run():
            transport = httpx.MockTransport(handler)
            async with torture.AsyncTorture(
                api_key="token", http_client=httpx.AsyncClient(transport=transport)
            ) as api:
                return await api.shapes.create(models.Circle(radius=1.5), max_retries=0)

        self.assertIsInstance(asyncio.run(run()), models.Circle)

    def test_a_bodiless_2xx_next_to_a_json_one_is_none(self) -> None:
        responses = (httpx.Response(204), httpx.Response(200, json={"id": "w", "name": "n"}))
        with client(self.respond(*responses)) as api:
            self.assertIsNone(api.widgets.update("w", name="n"))
            self.assertEqual(api.widgets.update("w", name="n"), models.Widget(id="w", name="n"))
        with client(self.respond(httpx.Response(204))) as api:
            raw = api.with_raw_response.widgets.update("w", name="n")
        self.assertEqual((raw.status_code, raw.parse()), (204, None))

        async def handler(request: httpx.Request) -> httpx.Response:
            return httpx.Response(204)

        async def run() -> object:
            transport = httpx.MockTransport(handler)
            async with torture.AsyncTorture(
                api_key="token", http_client=httpx.AsyncClient(transport=transport)
            ) as api:
                return await api.widgets.update("w", name="n")

        self.assertIsNone(asyncio.run(run()))

    def test_keyword_resources_are_escaped(self) -> None:
        reserved = SAMPLES["Reserved"]
        with client(self.respond(httpx.Response(200, json=reserved))) as api:
            # Its body is flattened into keyword arguments, escaped like the model fields.
            echoed = api.class_.reserved(
                class_="c",
                type="t",
                self_="s",
                value_1leading="1",
                with_space="w",
                properties={"k": "v"},
                extra="x",
                extra_fields_="e",
                additional_properties="a",
                any_properties="p",
            )
        self.assertEqual(echoed.class_, "c")
        self.assertEqual((echoed.extra_fields_, echoed.extra_fields), ("e", {}))
        self.assertEqual(json.loads(self.requests[0].content), reserved)


class PaginationTest(unittest.TestCase):
    PAGES = {
        "/items": {
            "1": {"result": {"items": [{"id": "a"}, {"id": "b"}], "meta": {"total_pages": 2}}},
            "2": {"result": {"items": [{"id": "c"}], "meta": {"total_pages": 2}}},
        },
        "/logs": {
            None: {"data": [{"id": "l1"}], "next": "n1", "has_more": True},
            "n1": {"data": [{"id": "l2"}], "has_more": False},
        },
    }

    def handler(self, request: httpx.Request) -> httpx.Response:
        param = request.url.params.get("page" if request.url.path == "/items" else "after")
        page = self.PAGES[request.url.path][param]
        return httpx.Response(200, json=page)

    def test_pages_and_their_items(self) -> None:
        transport = httpx.MockTransport(self.handler)
        api = paged.Paged(api_key="k", base_url="https://paged.test", http_client=httpx.Client(transport=transport))
        page = api.items.list()
        self.assertEqual([item.id for item in page.items], ["a", "b"])
        self.assertTrue(page.has_next_page())
        self.assertEqual(page.body.result.meta.total_pages, 2)
        last = page.get_next_page()
        self.assertEqual(([item.id for item in last.items], last.has_next_page()), (["c"], False))
        with self.assertRaises(RuntimeError):
            last.get_next_page()
        self.assertEqual([item.id for item in api.items.list()], ["a", "b", "c"])
        logs = api.logs.list()
        self.assertEqual([len(p.items) for p in logs.iter_pages()], [1, 1])
        self.assertEqual([item.id for item in logs], ["l1", "l2"])

    def test_async_pages(self) -> None:
        async def handler(request: httpx.Request) -> httpx.Response:
            return self.handler(request)

        async def run() -> tuple[list[str], list[str]]:
            transport = httpx.MockTransport(handler)
            async with paged.AsyncPaged(
                api_key="k", base_url="https://paged.test", http_client=httpx.AsyncClient(transport=transport)
            ) as api:
                first = await api.items.list()
                pages = [[item.id for item in p.items] async for p in first.iter_pages()]
                return [item.id async for item in api.logs.list()], pages[1]

        self.assertEqual(asyncio.run(run()), (["l1", "l2"], ["c"]))


class Chunks(httpx.SyncByteStream):
    def __init__(self, *chunks: bytes, fail: bool = False) -> None:
        self.chunks, self.fail = chunks, fail

    def __iter__(self):
        yield from self.chunks
        if self.fail:
            raise httpx.ReadError("reset")


class StreamTest(unittest.TestCase):
    def chat(self, stream: Chunks):
        requests: list[httpx.Request] = []

        def handler(request: httpx.Request) -> httpx.Response:
            requests.append(request)
            return httpx.Response(200, headers={"content-type": "text/event-stream"}, stream=stream)

        api = paged.Paged(api_key="k", http_client=httpx.Client(transport=httpx.MockTransport(handler)))
        return api.chat.create_stream(), requests

    def test_events_decode_into_models_until_done(self) -> None:
        events = b'event: chunk\nid: 7\ndata: {"id": "a"}\n\ndata: [DONE]\n\ndata: {"id": "b"}\n\n'
        stream, requests = self.chat(Chunks(events))
        with stream:
            self.assertEqual(list(stream), [paged.models.Item(id="a")])
        self.assertEqual((stream.last_event.event, stream.last_event.id), ("chunk", "7"))
        self.assertEqual(json.loads(requests[0].content), {"stream": True})

    def test_the_stream_twin_of_an_optional_body_sends_stream_alone(self) -> None:
        stream, requests = self.chat(Chunks(b"data: [DONE]\n\n"))
        with stream:
            self.assertEqual(list(stream), [])
        self.assertEqual(json.loads(requests[0].content), {"stream": True})
        for method in (paged.api.Chat.create, paged.api.Chat.create_stream):
            self.assertNotIn("stream", inspect.signature(method).parameters)

    def test_an_optional_body_left_empty_is_not_sent(self) -> None:
        requests: list[httpx.Request] = []

        def handler(request: httpx.Request) -> httpx.Response:
            requests.append(request)
            return httpx.Response(200, json={"id": "a"})

        api = paged.Paged(api_key="k", http_client=httpx.Client(transport=httpx.MockTransport(handler)))
        self.assertEqual(api.chat.create().id, "a")
        self.assertEqual(requests[0].content, b"")

    def test_a_connection_lost_mid_stream_is_an_sdk_error(self) -> None:
        stream, _ = self.chat(Chunks(b'data: {"id": "a"}\n\n', fail=True))
        with self.assertRaises(paged.APIConnectionError):
            list(stream)


class ScoresTest(unittest.TestCase):
    def setUp(self) -> None:
        self.requests: list[httpx.Request] = []

    def respond(self, *responses: httpx.Response):
        queue = list(responses)

        def handler(request: httpx.Request) -> httpx.Response:
            self.requests.append(request)
            return queue.pop(0) if len(queue) > 1 else queue[0]

        return handler

    def test_errors_are_typed_by_status_and_decode_the_declared_body(self) -> None:
        invalid = httpx.Response(
            422, json={"message": "bad", "fields": {"name": ["short"]}}, headers={"request-id": "r1"}
        )
        with client(self.respond(invalid)) as api:
            with self.assertRaises(UnprocessableEntityError) as raised:
                api.things.create(name="n", kind="alpha")
        self.assertIsInstance(raised.exception, APIStatusError)
        self.assertEqual(raised.exception.body, models.ValidationError(message="bad", fields={"name": ["short"]}))
        self.assertEqual(raised.exception.request_id, "r1")
        self.assertEqual(
            str(raised.exception),
            'Error code: 422 - {"message":"bad","fields":{"name":["short"]}}',
        )
        long = httpx.Response(404, text="x" * 600)
        with client(self.respond(long)) as api:
            with self.assertRaises(NotFoundError) as raised:
                api.things.retrieve("t")
        self.assertEqual(str(raised.exception), f"Error code: 404 - {'x' * 500}...")
        with client(self.respond(httpx.Response(404))) as api:
            with self.assertRaises(NotFoundError) as raised:
                api.things.retrieve("t")
        self.assertEqual(str(raised.exception), "Error code: 404 - Not Found")
        for status, error in ((404, NotFoundError), (500, InternalServerError), (418, APIStatusError)):
            with self.subTest(status=status):
                response = httpx.Response(status, json={"title": "t"})
                with client(self.respond(response), max_retries=0) as api:
                    with self.assertRaises(error) as raised:
                        api.widgets.create(name="n")
                self.assertIs(type(raised.exception), error)
                expected = models.Problem(title="t") if status < 500 else {"title": "t"}
                self.assertEqual(raised.exception.body, expected)
        with client(self.respond(httpx.Response(503, text="<html>")), max_retries=0) as api:
            with self.assertRaises(InternalServerError) as raised:
                api.widgets.list()
        self.assertIsNone(raised.exception.body)

    def test_503_is_not_replayed_for_non_idempotent_requests(self) -> None:
        unavailable = httpx.Response(503, headers={"retry-after": "0"})
        responses = (unavailable, unavailable, httpx.Response(200, json=THING))
        with client(self.respond(*responses), max_retries=1) as api:
            with self.assertRaises(InternalServerError):
                api.things.update("t")
            self.assertEqual(len(self.requests), 1)
            api.things.retrieve("t")
        self.assertEqual(len(self.requests), 3)

    def test_union_variants_default_their_discriminator(self) -> None:
        circle = models.Circle(radius=1.5)
        self.assertEqual(circle.to_dict(), SAMPLES["Shape"])
        activity = models.Activity(content=models.ActivityClosedVariant(by="b"))
        self.assertEqual((activity.kind, activity.to_dict()), ("closed", {"kind": "closed", "by": "b"}))
        reopened = models.Activity(kind="reopened", content=models.Opened(kind="reopened"))
        self.assertEqual(reopened.to_dict(), {"kind": "reopened"})
        unknown = models.Activity(content=UnknownVariant("archived", {"kind": "archived"}))
        self.assertEqual(unknown.kind, "archived")

    def test_primitive_or_object_unions_are_typed(self) -> None:
        Charge = expandable.models.Charge
        for data in (
            {"customer": "cus_1", "shipping": "", "tags": True},
            {"customer": {"id": "cus_1"}, "shipping": {"name": "n"}, "tags": ["a"]},
        ):
            with self.subTest(data=data):
                self.assertEqual(Charge.from_dict(data).to_dict(), data)
        charge = Charge.from_dict({"customer": {"id": "cus_1"}, "shipping": {"name": "n"}})
        self.assertEqual(charge.customer, expandable.models.Customer(id="cus_1"))
        self.assertEqual(charge.shipping, expandable.models.Shipping(name="n"))
        with self.assertRaises(expandable.serialization.ModelParseError):
            Charge.from_dict({"customer": 1})
        self.assertEqual(round_trip(torture, "StringOrInt", 3), 3)
        self.assertEqual(round_trip(torture, "StringOrInt", "3"), "3")

    def test_resource_method_names(self) -> None:
        transport = httpx.MockTransport(self.respond(httpx.Response(200, json=THING)))
        with Torture(api_key="token", http_client=httpx.Client(transport=transport)) as api:
            thing = api.things.retrieve("a/b")
            api.things.update("a/b")
        self.assertEqual(thing.id, "a/b")
        self.assertEqual([r.method for r in self.requests], ["GET", "PATCH"])


if __name__ == "__main__":
    unittest.main()

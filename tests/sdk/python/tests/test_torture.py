"""Generates tests/fixtures/torture.yaml, then round-trips its models and drives the client."""

import asyncio
import importlib
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


def generate(name: str, python: str = "", spec: str | None = None) -> object:
    """Generates the fixture, or `spec`, as the `name` package and imports it."""
    fixtures = os.environ.get("FIXTURES")
    assert fixtures, "set FIXTURES to the perseid tests/fixtures directory"
    project = WORK / name
    project.mkdir()
    if spec is None:
        shutil.copy(Path(fixtures) / "torture.yaml", project / "openapi.yaml")
    else:
        (project / "openapi.yaml").write_text(spec)
    (project / "perseid.toml").write_text(
        f'spec = "openapi.yaml"\nname = "{name.title()}"\nsdks = ["python"]\n'
        f'base_url = "https://torture.test/v1"\n[python]\n{python}'
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

torture = generate("torture")
expandable = generate("expandable", spec=EXPANDABLE_SPEC)
flat = generate("flat", "flat_unions = true\n")
adjacent = generate("adjacent", spec=ADJACENT_SPEC)
from torture import Torture, TortureOptions, models  # noqa: E402
from torture.api import (  # noqa: E402
    ApiStatusError,
    InternalServerError,
    NotFoundError,
    UnprocessableEntityError,
)
from torture.errors import ApiException, NetworkException  # noqa: E402
from torture.serialization import UNSET, UnknownVariant, from_json_value  # noqa: E402
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
    "Reserved": {"class": "c", "type": "t", "self": "s", "1leading": "1", "with space": "w"},
    "Widget": {"id": "w", "name": "n", "reactions": {"+1": 1, "-1": 2}},
}


def round_trip(package: object, model: str, data: object) -> object:
    annotation = getattr(package.models, model)
    parsed = package.serialization.from_json_value(annotation, data)
    return json.loads(json.dumps(package.serialization.to_json_value(parsed, annotation)))


class ModelTest(unittest.TestCase):
    def test_models_round_trip_in_both_union_styles(self) -> None:
        for package in (torture, flat):
            for model, data in SAMPLES.items():
                with self.subTest(package=package.__name__, model=model):
                    self.assertEqual(round_trip(package, model, data), data)

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
        shape = models.Shape.from_dict(data)
        self.assertEqual(shape.content, UnknownVariant("triangle", data))
        self.assertEqual(shape.to_dict(), data)
        unknown = flat.serialization.from_json_value(flat.models.Shape, data)
        self.assertEqual(unknown, flat.serialization.UnknownVariant("triangle", data))
        self.assertEqual(round_trip(flat, "Shape", data), data)

    def test_flat_unions_are_the_variant_models(self) -> None:
        serialization = flat.serialization
        shape = serialization.from_json_value(flat.models.Shape, SAMPLES["Shape"])
        self.assertIsInstance(shape, flat.models.Circle)
        square = flat.models.Square(side=2, type="ignored")
        serialized = serialization.to_json_value(square, flat.models.Shape)
        self.assertEqual(serialized, {"side": 2, "type": "square"})

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
        self.assertEqual(source.note, adjacent.models.UNSET)

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
        self.assertEqual(unions.loose, {"title": "t"})
        self.assertEqual(unions.to_dict(), payload)
        draft = models.ObjectUnions.from_dict({"document": {"title": "t"}}).document
        self.assertIsInstance(draft, models.Draft, "ties go to the first variant")
        untitled = models.ObjectUnions.from_dict({"document": {"body": "b"}}).document
        self.assertIsInstance(untitled, UnknownVariant)

    def test_recursive_models_import_and_parse(self) -> None:
        tree = models.TreeNode.from_dict(SAMPLES["TreeNode"])
        self.assertEqual(tree.children[0].value, "c")
        self.assertIsInstance(tree.next, models.TreeNode)


def client(handler, **options) -> Torture:
    transport = httpx.MockTransport(handler)
    return Torture(
        "token",
        TortureOptions(timeout=5, **options),
        httpx.Client(transport=transport),
    )


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

    def test_list_bodies_and_odd_query_names(self) -> None:
        widget = {"id": "w", "name": "n"}
        with client(self.respond(httpx.Response(200, json=[widget]))) as api:
            created = api.widgets.bulk([models.WidgetUpdate(name="n")])
            api.widgets.list(date_created_lt="2024-01-01")
        self.assertEqual(created, [models.Widget(id="w", name="n")])
        self.assertEqual(json.loads(self.requests[0].content), [{"name": "n"}])
        self.assertEqual(self.requests[1].url.params["DateCreated<"], "2024-01-01")

    def test_a_user_idempotency_key_replaces_the_automatic_one(self) -> None:
        create = models.ThingCreate(name="n", kind=models.Kind.ALPHA)
        with client(self.respond(httpx.Response(200, json=THING))) as api:
            api.things.create(create)
            api.things.create(create, idempotency_key="mine")
            api.things.create(create, extra_headers={"IDEMPOTENCY-KEY": "extra"})
        keys = [r.headers.get_list("idempotency-key") for r in self.requests]
        self.assertTrue(keys[0][0].startswith("auto_"))
        self.assertEqual(keys[1:], [["mine"], ["extra"]])

    def test_patch_sends_explicit_nulls(self) -> None:
        with client(self.respond(httpx.Response(200, json=THING))) as api:
            api.things.update("t", models.ThingPatch(name="n", description=None))
        self.assertEqual(json.loads(self.requests[0].content), {"name": "n", "description": None})

    def test_429_is_retried_after_the_delay_the_server_asks_for(self) -> None:
        responses = (
            httpx.Response(429, headers={"retry-after": "0.2"}),
            httpx.Response(200, json=THING),
        )
        with client(self.respond(*responses), retry_schedule=[0.0]) as api:
            started = time.monotonic()
            api.things.update("t", models.ThingPatch())
        self.assertGreaterEqual(time.monotonic() - started, 0.2)
        self.assertEqual(self.requests[1].headers["torture-retry-count"], "1")

    def test_non_idempotent_requests_are_not_replayed_on_5xx(self) -> None:
        responses = (
            httpx.Response(500, headers={"x-request-id": "req_1"}),
            httpx.Response(500),
            httpx.Response(200, json=THING),
        )
        with client(self.respond(*responses), retry_schedule=[0.0]) as api:
            with self.assertRaises(ApiException) as raised:
                api.things.update("t", models.ThingPatch())
            self.assertEqual(raised.exception.headers["x-request-id"], "req_1")
            self.assertEqual(len(self.requests), 1)
            api.things.create(models.ThingCreate(name="n", kind=models.Kind.ALPHA))
        self.assertEqual(len(self.requests), 3)
        self.assertEqual(self.requests[1].headers["idempotency-key"],
                         self.requests[2].headers["idempotency-key"])

    def test_timeouts_are_retried_then_raised(self) -> None:
        def handler(request: httpx.Request) -> httpx.Response:
            self.requests.append(request)
            raise httpx.ReadTimeout("slow", request=request)

        with client(handler, retry_schedule=[0.0, 0.0]) as api:
            with self.assertRaises(NetworkException):
                api.things.retrieve("t")
        self.assertEqual(len(self.requests), 3)

    def test_async_client(self) -> None:
        async def handler(request: httpx.Request) -> httpx.Response:
            return httpx.Response(200, json=SAMPLES["Shape"])

        async def run():
            transport = httpx.MockTransport(handler)
            async with torture.TortureAsync(
                "token", httpx_client=httpx.AsyncClient(transport=transport)
            ) as api:
                return await api.shapes.create(
                    models.Shape(type="circle", content=models.Circle(radius=1.5, type="circle"))
                )

        shape = asyncio.run(run())
        self.assertIsInstance(shape.content, models.Circle)

    def test_keyword_resources_are_escaped(self) -> None:
        reserved = SAMPLES["Reserved"]
        with client(self.respond(httpx.Response(200, json=reserved))) as api:
            echoed = api.class_.reserved(models.Reserved.from_dict(reserved))
        self.assertEqual(echoed.class_, "c")
        self.assertEqual(json.loads(self.requests[0].content), reserved)


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
        create = models.ThingCreate(name="n", kind=models.Kind.ALPHA)
        invalid = httpx.Response(
            422, json={"message": "bad", "fields": {"name": ["short"]}}, headers={"request-id": "r1"}
        )
        with client(self.respond(invalid)) as api:
            with self.assertRaises(UnprocessableEntityError) as raised:
                api.things.create(create)
        self.assertIsInstance(raised.exception, ApiException)
        self.assertEqual(raised.exception.body, models.ValidationError(message="bad", fields={"name": ["short"]}))
        self.assertEqual(raised.exception.request_id, "r1")
        for status, error in ((404, NotFoundError), (500, InternalServerError), (418, ApiStatusError)):
            with self.subTest(status=status):
                response = httpx.Response(status, json={"title": "t"})
                with client(self.respond(response), num_retries=0) as api:
                    with self.assertRaises(error) as raised:
                        api.widgets.create(models.WidgetUpdate(name="n"))
                self.assertIs(type(raised.exception), error)
                expected = models.Problem(title="t") if status < 500 else None
                self.assertEqual(raised.exception.body, expected)
        with client(self.respond(httpx.Response(503, text="<html>")), num_retries=0) as api:
            with self.assertRaises(InternalServerError) as raised:
                api.widgets.list()
        self.assertIsNone(raised.exception.body)

    def test_503_is_not_replayed_for_non_idempotent_requests(self) -> None:
        unavailable = httpx.Response(503, headers={"retry-after": "0"})
        responses = (unavailable, unavailable, httpx.Response(200, json=THING))
        with client(self.respond(*responses), retry_schedule=[0.0]) as api:
            with self.assertRaises(InternalServerError):
                api.things.update("t", models.ThingPatch())
            self.assertEqual(len(self.requests), 1)
            api.things.retrieve("t")
        self.assertEqual(len(self.requests), 3)

    def test_union_variants_default_their_discriminator(self) -> None:
        circle = models.Circle(radius=1.5)
        self.assertEqual(circle.to_dict(), SAMPLES["Shape"])
        shape = models.Shape(content=circle)
        self.assertEqual((shape.type, shape.to_dict()), ("circle", SAMPLES["Shape"]))
        unknown = models.Shape(content=UnknownVariant("hexagon", {"type": "hexagon"}))
        self.assertEqual(unknown.type, "hexagon")

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
        with Torture("token", httpx_client=httpx.Client(transport=transport)) as api:
            thing = api.things.retrieve("a/b")
            api.things.update("a/b", models.ThingPatch())
        self.assertEqual(thing.id, "a/b")
        self.assertEqual([r.method for r in self.requests], ["GET", "PATCH"])


if __name__ == "__main__":
    unittest.main()

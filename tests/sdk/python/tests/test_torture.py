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


def generate(name: str, context: str = "", spec: str | None = None) -> object:
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
        f'spec = "openapi.yaml"\nname = "{name.title()}"\n'
        f'base_url = "https://torture.test/v1"\n[python]\n{context}'
    )
    for command in (["init"], ["generate"]):
        subprocess.run(["perseid", *command], cwd=project, check=True, capture_output=True)
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

torture = generate("torture")
flat = generate("flat", "[python.context]\nflat_unions = true\n")
adjacent = generate("adjacent", spec=ADJACENT_SPEC)
from torture import Torture, TortureOptions, models  # noqa: E402
from torture.errors import ApiException, NetworkException  # noqa: E402
from torture.serialization import UNSET, UnknownVariant, from_json_value  # noqa: E402

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
            thing = api.things.get_thing("a/b", extra_headers={"X-Extra": "1"}, timeout=2)
        request = self.requests[0]
        self.assertEqual(thing.id, "a/b")
        self.assertEqual(request.url.raw_path, b"/v1/things/a%2Fb")
        self.assertEqual(request.headers["x-extra"], "1")
        self.assertEqual(request.headers["authorization"], "Bearer token")
        self.assertEqual(request.extensions["timeout"]["read"], 2)

    def test_query_and_header_params(self) -> None:
        with client(self.respond(httpx.Response(200, json={"data": [], "total": 0}))) as api:
            api.things.list_things(
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
            created = api.widgets.bulk_create_widgets([models.WidgetUpdate(name="n")])
            api.widgets.list_widgets(date_created_lt="2024-01-01")
        self.assertEqual(created, [models.Widget(id="w", name="n")])
        self.assertEqual(json.loads(self.requests[0].content), [{"name": "n"}])
        self.assertEqual(self.requests[1].url.params["DateCreated<"], "2024-01-01")

    def test_a_user_idempotency_key_replaces_the_automatic_one(self) -> None:
        create = models.ThingCreate(name="n", kind=models.Kind.ALPHA)
        with client(self.respond(httpx.Response(200, json=THING))) as api:
            api.things.create_thing(create)
            api.things.create_thing(create, idempotency_key="mine")
            api.things.create_thing(create, extra_headers={"IDEMPOTENCY-KEY": "extra"})
        keys = [r.headers.get_list("idempotency-key") for r in self.requests]
        self.assertTrue(keys[0][0].startswith("auto_"))
        self.assertEqual(keys[1:], [["mine"], ["extra"]])

    def test_patch_sends_explicit_nulls(self) -> None:
        with client(self.respond(httpx.Response(200, json=THING))) as api:
            api.things.update_thing("t", models.ThingPatch(name="n", description=None))
        self.assertEqual(json.loads(self.requests[0].content), {"name": "n", "description": None})

    def test_429_is_retried_after_the_delay_the_server_asks_for(self) -> None:
        responses = (
            httpx.Response(429, headers={"retry-after": "0.2"}),
            httpx.Response(200, json=THING),
        )
        with client(self.respond(*responses), retry_schedule=[0.0]) as api:
            started = time.monotonic()
            api.things.update_thing("t", models.ThingPatch())
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
                api.things.update_thing("t", models.ThingPatch())
            self.assertEqual(raised.exception.headers["x-request-id"], "req_1")
            self.assertEqual(len(self.requests), 1)
            api.things.create_thing(models.ThingCreate(name="n", kind=models.Kind.ALPHA))
        self.assertEqual(len(self.requests), 3)
        self.assertEqual(self.requests[1].headers["idempotency-key"],
                         self.requests[2].headers["idempotency-key"])

    def test_timeouts_are_retried_then_raised(self) -> None:
        def handler(request: httpx.Request) -> httpx.Response:
            self.requests.append(request)
            raise httpx.ReadTimeout("slow", request=request)

        with client(handler, retry_schedule=[0.0, 0.0]) as api:
            with self.assertRaises(NetworkException):
                api.things.get_thing("t")
        self.assertEqual(len(self.requests), 3)

    def test_async_client(self) -> None:
        async def handler(request: httpx.Request) -> httpx.Response:
            return httpx.Response(200, json=SAMPLES["Shape"])

        async def run():
            transport = httpx.MockTransport(handler)
            async with torture.TortureAsync(
                "token", httpx_client=httpx.AsyncClient(transport=transport)
            ) as api:
                return await api.shapes.echo_shape(
                    models.Shape(type="circle", content=models.Circle(radius=1.5, type="circle"))
                )

        shape = asyncio.run(run())
        self.assertIsInstance(shape.content, models.Circle)

    def test_keyword_resources_are_escaped(self) -> None:
        reserved = SAMPLES["Reserved"]
        with client(self.respond(httpx.Response(200, json=reserved))) as api:
            echoed = api.class_.echo_reserved(models.Reserved.from_dict(reserved))
        self.assertEqual(echoed.class_, "c")
        self.assertEqual(json.loads(self.requests[0].content), reserved)


if __name__ == "__main__":
    unittest.main()

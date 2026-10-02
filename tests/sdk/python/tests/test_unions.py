"""Unions of the SDK generated from tests/fixtures/edge-unions.yaml: members sharing a JSON type,
object unions without a discriminator, union bodies and open enums, decoded and encoded."""

import json
import os
import unittest
from datetime import datetime

import httpx

from _support import generate_fixture, import_package

edge = import_package(generate_fixture("edge-unions.yaml", "Edgeunions"))
models = edge.models

os.environ.pop("EDGEUNIONS_API_KEY", None)
os.environ.pop("EDGEUNIONS_BASE_URL", None)
os.environ.pop("EDGEUNIONS_CLIENT_ID", None)
os.environ.pop("EDGEUNIONS_CLIENT_SECRET", None)

BASE = "https://union.test/v1"


def completion(**fields: object) -> dict:
    return {"id": "c1", "model": "alpha-1", "created_at": "2024-05-01T10:00:00Z", "choices": [], **fields}


class Server:
    """Answers every call with `body` and records the requests."""

    def __init__(self, body: object) -> None:
        self.body = body
        self.calls: list[httpx.Request] = []

    def __call__(self, request: httpx.Request) -> httpx.Response:
        self.calls.append(request)
        if request.url.path.endswith("/oauth/token"):
            return httpx.Response(200, json={"access_token": "tok", "token_type": "Bearer", "expires_in": 3600})
        return httpx.Response(200, json=self.body)

    def client(self):
        http = httpx.Client(transport=httpx.MockTransport(self))
        return edge.Edgeunions(
            http_client=http, base_url=BASE, client_id="id", client_secret="secret", max_retries=0
        )

    @property
    def sent(self) -> object:
        return json.loads(self.calls[-1].content)


class SharedJsonTypeTest(unittest.TestCase):
    def prompt(self, value: object) -> object:
        return models.Completion.from_dict(completion(prompt=value)).prompt

    def test_the_variant_is_told_apart_by_the_items(self) -> None:
        self.assertEqual(self.prompt("hi"), "hi")
        self.assertEqual(self.prompt(["a", "b"]), ["a", "b"])
        self.assertEqual(self.prompt([1, 2]), [1, 2])
        self.assertEqual(self.prompt([[1], [2, 3]]), [[1], [2, 3]])
        self.assertEqual(self.prompt([]), [])

    def test_a_date_time_falls_back_to_a_string(self) -> None:
        stamped = models.Completion.from_dict(completion())
        self.assertIsInstance(stamped.created_at, datetime)
        loose = models.Completion.from_dict(completion(created_at="yesterday"))
        self.assertEqual(loose.created_at, "yesterday")
        self.assertIsNone(models.Completion.from_dict(completion(expires_at=None)).expires_at)

    def test_integers_and_numbers_share_one_variant(self) -> None:
        self.assertEqual(models.Completion.from_dict(completion(score=3)).score, 3)
        self.assertEqual(models.Completion.from_dict(completion(score=1.5)).score, 1.5)
        self.assertEqual(models.Completion.from_dict(completion(stop=2)).stop, 2)
        self.assertEqual(models.Completion.from_dict(completion(stop="x")).stop, "x")

    def test_requests_encode_the_variant_the_caller_holds(self) -> None:
        for prompt in ("hi", ["a"], [1, 2], [[1], [2]]):
            request = models.CreateCompletionRequest(model="alpha-1", prompt=prompt)
            self.assertEqual(request.to_dict()["prompt"], prompt)
        stamped = models.CreateCompletionRequest(
            model="alpha-1", prompt="p", created_after=datetime(2024, 5, 1, 10, 0, 0).astimezone()
        )
        self.assertIsInstance(stamped.to_dict()["created_after"], str)

    def test_a_request_round_trips_through_the_server(self) -> None:
        server = Server(completion(prompt=[[1, 2]]))
        with server.client() as api:
            done = api.completions.create(model="alpha-1", prompt=[[1, 2]])
        self.assertEqual(server.sent["prompt"], [[1, 2]])
        self.assertEqual(done.prompt, [[1, 2]])


class OpenEnumTest(unittest.TestCase):
    def test_unknown_values_are_kept(self) -> None:
        known = models.Completion.from_dict(completion(model="alpha-2"))
        custom = models.Completion.from_dict(completion(model="my-fine-tune"))
        self.assertEqual(known.model, "alpha-2")
        self.assertEqual(custom.model, "my-fine-tune")
        self.assertEqual(custom.to_dict()["model"], "my-fine-tune")
        self.assertEqual(
            models.Completion.from_dict(completion(include=["usage", "future"])).to_dict()["include"],
            ["usage", "future"],
        )

    def test_a_request_takes_plain_strings(self) -> None:
        request = models.CreateCompletionRequest(model="brand-new", prompt="p", voice="alloy")
        self.assertEqual(request.to_dict()["model"], "brand-new")
        self.assertEqual(request.to_dict()["voice"], "alloy")


class ObjectUnionTest(unittest.TestCase):
    def choice(self, value: object) -> object:
        return models.Response.from_dict({"id": "r", "model": "alpha-1", "tool_choice": value}).tool_choice

    def test_objects_are_decoded_as_their_best_matching_variant(self) -> None:
        self.assertEqual(self.choice("auto"), "auto")
        self.assertIsInstance(self.choice({"mode": "auto", "tools": []}), models.AllowedTools)
        self.assertIsInstance(self.choice({"type": "web_search"}), models.HostedTool)
        picked = self.choice({"name": "lookup", "arguments": "{}"})
        self.assertIsInstance(picked, models.FunctionTool)
        self.assertEqual(picked.name, "lookup")

    def test_an_object_no_variant_decodes_is_kept(self) -> None:
        kept = self.choice({"unexpected": 1})
        self.assertNotIsInstance(kept, (models.AllowedTools, models.HostedTool, models.FunctionTool))

    def test_a_variant_encodes_as_itself(self) -> None:
        request = models.CreateResponseRequest(model="m", tool_choice=models.FunctionTool(name="f"))
        self.assertEqual(request.to_dict()["tool_choice"], {"name": "f"})
        request = models.CreateResponseRequest(model="m", input=[models.HostedTool(type="web_search")])
        self.assertEqual(request.to_dict()["input"], [{"type": "web_search"}])


class UnionBodyTest(unittest.TestCase):
    def test_a_union_response_body_is_decoded_as_its_best_matching_variant(self) -> None:
        with Server({"text": "hello"}).client() as api:
            plain = api.transcriptions.create(model="alpha-1", file_id="f")
        self.assertIsInstance(plain, models.Transcription)
        verbose = {"text": "hello", "duration": 1.5, "language": "fr"}
        with Server(verbose).client() as api:
            full = api.transcriptions.create(model="alpha-1", file_id="f")
        self.assertIsInstance(full, models.TranscriptionVerbose)
        self.assertEqual(full.language, "fr")

    def test_a_list_of_unions_is_decoded_item_by_item(self) -> None:
        body = [{"text": "a"}, {"text": "b", "duration": 2, "language": "en"}]
        with Server(body).client() as api:
            items = api.transcriptions.list()
        self.assertEqual([type(i) for i in items], [models.Transcription, models.TranscriptionVerbose])

    def test_a_union_request_body_is_sent_as_the_variant_it_holds(self) -> None:
        server = Server({"id": "g"})
        with server.client() as api:
            api.grades.create(models.GradeByScore(score=0.5))
            self.assertEqual(server.sent, {"score": 0.5})
            api.grades.create(models.GradeByText(text="ok", reference="ok"))
            self.assertEqual(server.sent, {"text": "ok", "reference": "ok"})


class RequiredOnlyTest(unittest.TestCase):
    def test_required_only_alternatives_leave_a_model(self) -> None:
        ref = models.ImageRef(file_id="f")
        self.assertEqual(ref.to_dict(), {"file_id": "f"})
        batch = models.CreateFileBatchRequest(file_ids=["a"])
        self.assertEqual(batch.to_dict(), {"file_ids": ["a"]})


if __name__ == "__main__":
    unittest.main()

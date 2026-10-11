"""The CLI generated from tests/acme, run against tests/mock.py: `ACME=path/to/acme python3 e2e.py`.

Golden `--help` outputs live in tests/golden; `UPDATE_GOLDEN=1` rewrites them.
"""

import base64
import json
import os
import shlex
import socket
import subprocess
import sys
import tempfile
import time
import unittest
import urllib.request
from pathlib import Path
from urllib.parse import parse_qs, urlparse

HERE = Path(__file__).resolve().parent
ACME = os.environ.get("ACME", "acme")
HELPS = [
    [],
    ["things"],
    ["things", "list"],
    ["things", "create"],
    ["things", "retrieve"],
    ["things", "create-photo"],
    ["things", "attachments"],
    ["things", "attachments", "upload"],
    ["things", "attachments", "download"],
    ["things", "retrieve-batch"],
    ["reports", "retrieve"],
    ["attachment"],
    ["schema"],
    ["auth-api"],
    ["auth", "login"],
    ["mcp"],
]


def has_constraints():
    """Whether perseid's model has constraints (perseid#113): goldens and their tests need them."""
    env = {"PATH": os.environ.get("PATH", ""), "ACME_NO_KEYRING": "1"}
    out = subprocess.run([ACME, "schema", "things", "list"], env=env, capture_output=True).stdout
    return "maximum" in json.loads(out or "{}").get("parameters", {}).get("properties", {}).get("--limit", {})


CONSTRAINTS = has_constraints()
WITHOUT_CONSTRAINTS = "perseid's model has no constraints: goldens and these tests need perseid#113"


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


class Cli(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.port = free_port()
        cls.server = subprocess.Popen([sys.executable, "-I", str(HERE / "mock.py"), str(cls.port)])
        for _ in range(100):
            try:
                socket.create_connection(("127.0.0.1", cls.port), timeout=0.1).close()
                break
            except OSError:
                time.sleep(0.05)
        cls.home = tempfile.TemporaryDirectory()

    @classmethod
    def tearDownClass(cls):
        cls.server.terminate()
        cls.server.wait()
        cls.home.cleanup()

    def setUp(self):
        self.mark = len(self.requests())

    def requests(self):
        with urllib.request.urlopen(f"http://127.0.0.1:{self.port}/__requests") as response:
            return json.load(response)

    def sent(self):
        """The requests of this test."""
        return self.requests()[self.mark:]

    def env(self, key="sk_test", home=None, extra=None):
        env = {
            "PATH": os.environ.get("PATH", ""),
            "XDG_CONFIG_HOME": home or self.home.name,
            "ACME_BASE_URL": f"http://127.0.0.1:{self.port}",
        }
        if key:
            env["ACME_API_KEY"] = key
        env["ACME_NO_KEYRING"] = "1"
        return {k: v for k, v in (env | (extra or {})).items() if v is not None}

    def run_cli(self, *args, key="sk_test", stdin=None, home=None, env=None):
        return subprocess.run(
            [ACME, *args], env=self.env(key, home, env), input=stdin, capture_output=True, timeout=60,
            cwd=self.home.name
        )

    def ok(self, *args, **kwargs):
        result = self.run_cli(*args, **kwargs)
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        return result.stdout.decode()

    def tty(self, *args, env=None):
        """Runs the CLI on a terminal, through `script`: stdout and stderr, merged."""
        command = shlex.join([ACME, *args])
        result = subprocess.run(["script", "-qec", command, "/dev/null"], env={**self.env(), **(env or {})},
                                stdin=subprocess.DEVNULL, capture_output=True, timeout=60, cwd=self.home.name)
        self.assertEqual(result.returncode, 0, result.stdout.decode())
        return result.stdout.decode().replace("\r\n", "\n")

    def error(self, code, *args, **kwargs):
        result = self.run_cli(*args, **kwargs)
        self.assertEqual(result.returncode, code, result.stderr.decode())
        self.assertEqual(result.stdout, b"")
        return json.loads(result.stderr)["error"]

    @unittest.skipUnless(CONSTRAINTS, WITHOUT_CONSTRAINTS)
    def test_help_is_golden(self):
        update = os.environ.get("UPDATE_GOLDEN")
        for command in HELPS:
            path = HERE / "golden" / ("-".join(command or ["acme"]) + ".txt")
            help = self.ok(*command, "--help")
            if update:
                path.parent.mkdir(exist_ok=True)
                path.write_text(help)
            self.assertEqual(help, path.read_text(), f"{path.name}: UPDATE_GOLDEN=1 rewrites it")

    def test_lists_stream_every_page(self):
        lines = self.ok("things", "list").splitlines()
        self.assertEqual([json.loads(line)["id"] for line in lines], [f"th_{i}" for i in range(1, 6)])
        cursors = [r["query"].get("starting_after") for r in self.sent()]
        self.assertEqual(cursors, [None, ["th_2"], ["th_4"]])

    def test_max_items_stops_paging(self):
        out = self.ok("things", "list", "--max-items", "3", "--jq", ".name")
        self.assertEqual(out, "Widget\nGadget\nGizmo\n")
        self.assertEqual(len(self.sent()), 2)

    def test_query_flags_are_typed(self):
        self.ok("things", "list", "--limit", "5", "--status", "draft", "--status", "active",
                "--query-format", "short", "--archived")
        query = self.sent()[0]["query"]
        self.assertEqual(query["limit"], ["5"])
        self.assertEqual(query["status"], ["draft", "active"])
        self.assertEqual(query["format"], ["short"])
        self.assertEqual(query["archived"], ["true"])
        clap = self.error(2, "things", "list", "--status", "gone")
        self.assertIn("invalid value 'gone' for '--status <STRING,...>'", clap["message"])
        self.assertEqual(self.error(2, "things", "list", "--limit", "ten")["kind"], "usage")

    def test_retrieve_takes_the_path_parameter(self):
        thing = json.loads(self.ok("things", "retrieve", "th_3", "--format", "json"))
        self.assertEqual(thing["name"], "Gizmo")
        self.assertEqual(self.sent()[0]["path"], "/v1/things/th_3")

    def test_create_merges_data_and_flags(self):
        out = self.ok("things", "create", "--name", "Lamp", "--quantity", "3", "--active",
                      "--status", "active", "--tags", "new", "--tags", "sale", "--body-format", "plain",
                      "-d", '{"name": "ignored", "address": {"city": "Paris"}}',
                      "--idempotency-key", "k1")
        self.assertEqual(json.loads(out)["name"], "Lamp")
        request = self.sent()[0]
        self.assertEqual(request["headers"]["idempotency-key"], "k1")
        self.assertEqual(json.loads(request["body"]), {
            "name": "Lamp", "quantity": 3, "active": True, "status": "active",
            "tags": ["new", "sale"], "address": {"city": "Paris"}, "format": "plain",
        })
        body = json.loads(self.ok("things", "create", "-d", "-", stdin=b'{"name": "Stdin"}'))
        self.assertEqual(body["name"], "Stdin")

    def test_invalid_bodies_are_usage_errors(self):
        missing = self.error(2, "things", "create", "--quantity", "3")
        self.assertEqual(missing["message"], "invalid body: name (--name): required (a string)")
        self.assertEqual(missing["errors"], [{"path": "name", "flag": "--name", "message": "required (a string)"}])
        self.assertEqual(self.error(2, "things", "create", "--quantity", "many")["kind"], "usage")
        self.assertEqual(self.sent(), [])

    def test_bodies_are_checked_against_the_schema(self):
        body = '{"name": "Lamp", "address": {"city": "Paris", "zip": "75001"}, "status": "gone", "since": "31/01"}'
        errors = self.error(2, "things", "create", "-d", body)["errors"]
        self.assertEqual([(e["path"], e["flag"], e["message"]) for e in errors], [
            ("address.zip", "--address", "unknown field; expected one of city, country"),
            ("status", "--status", "unknown value `gone`; expected one of draft, active, retired"),
            ("since", "--since", "invalid value `31/01`: expected a date, YYYY-MM-DD"),
        ])
        typo = self.error(2, "things", "create", "--name", "Lamp", "-d", '{"quantty": 1}')
        self.assertEqual(typo["message"], "invalid body: quantty: unknown field; did you mean quantity?")
        self.assertEqual(self.sent(), [])
        self.ok("things", "create", "--name", "Lamp", "-d", '{"quantty": 1}', "--allow-unknown-fields")
        self.assertEqual(json.loads(self.sent()[0]["body"])["quantty"], 1)

    def test_schema_describes_every_command(self):
        index = json.loads(self.ok("schema"))
        self.assertIn({"command": "things create", "method": "POST", "path": "/v1/things",
                       "summary": "Create a thing"}, index)
        self.assertEqual([c["command"] for c in json.loads(self.ok("schema", "things", "attachments"))],
                         ["things attachments upload", "things attachments download"])
        create = json.loads(self.ok("schema", "things", "create"))
        self.assertEqual(create["body"]["required"], ["name"])
        self.assertEqual(create["body"]["properties"]["status"]["enum"], ["draft", "active", "retired"])
        self.assertEqual(create["body"]["properties"]["address"]["properties"]["city"], {"type": "string"})
        self.assertEqual(create["parameters"]["properties"]["--idempotency-key"]["in"], "header")
        self.assertEqual(create["example"], {"name": "Lamp"})
        listed = json.loads(self.ok("schema", "things", "list", "--response"))
        self.assertEqual((listed["output"], listed["pagination"]["arg"]), ("jsonl", "--starting-after"))
        self.assertEqual(listed["response"]["required"], ["id", "name", "quantity", "status", "tags"])
        self.assertEqual(self.error(2, "schema", "nothing")["kind"], "usage")
        self.assertLess(len(self.ok("agents")), 2048)

    @unittest.skipUnless(CONSTRAINTS, WITHOUT_CONSTRAINTS)
    def test_parameters_are_checked_against_their_constraints(self):
        limit = self.error(2, "things", "list", "--limit", "500")
        self.assertEqual(limit["message"], "invalid parameters: limit (--limit): must be at most 100")
        self.assertEqual(limit["errors"], [{"path": "limit", "flag": "--limit", "message": "must be at most 100"}])
        self.assertEqual(self.error(2, "things", "list", "--limit", "0", "--dry-run")["errors"][0]["message"],
                         "must be at least 1")
        self.assertEqual(self.error(2, "things", "retrieve", "at_1")["message"],
                         "invalid parameters: thing_id (<THING_ID>): must match the pattern ^th_")
        self.assertEqual(self.error(2, "things", "retrieve-batch", "1,0")["errors"],
                         [{"path": "ids[1]", "flag": "<IDS>", "message": "must be at least 1"}])
        self.assertEqual(self.sent(), [])
        self.ok("things", "list", "--limit", "100", "--max-items", "1")
        self.assertEqual(self.sent()[0]["query"]["limit"], ["100"])

    @unittest.skipUnless(CONSTRAINTS, WITHOUT_CONSTRAINTS)
    def test_bodies_are_checked_against_their_constraints(self):
        errors = self.error(2, "things", "create", "--name", "", "--quantity", "2000", "--contact", "jane@",
                            "--website", "/lamp", "--price", "1.005", "--tags", "a", "--tags", "a",
                            "--sku", "abc-1")["errors"]
        self.assertEqual([(e["path"], e["flag"], e["message"]) for e in errors], [
            ("contact", "--contact", "invalid value `jane@`: expected an email address"),
            ("name", "--name", "must be at least 1 character long"),
            ("price", "--price", "must be a multiple of 0.01"),
            ("quantity", "--quantity", "must be at most 1000"),
            ("sku", "--sku", "must match the pattern ^[A-Z]{3}-[0-9]+$"),
            ("tags", "--tags", "must not repeat `a`"),
            ("website", "--website", "invalid value `/lamp`: expected an absolute URI, such as https://example.com"),
        ])
        self.assertEqual(self.sent(), [])
        self.ok("things", "create", "--name", "Lamp", "--contact", "jane@acme.dev", "--website", "https://acme.dev",
                "--price", "19.99", "--sku", "ABC-1", "--tags", "a", "--tags", "b")
        self.assertEqual(json.loads(self.sent()[0]["body"])["sku"], "ABC-1")

    @unittest.skipUnless(CONSTRAINTS, WITHOUT_CONSTRAINTS)
    def test_constraints_are_in_the_schema_and_mcp_tools(self):
        listed = json.loads(self.ok("schema", "things", "list"))["parameters"]["properties"]
        limit = {k: listed["--limit"][k] for k in ["minimum", "maximum", "default", "examples"]}
        self.assertEqual(limit, {"minimum": 1, "maximum": 100, "default": 20, "examples": [10]})
        self.assertTrue(listed["--query-format"]["deprecated"])
        body = json.loads(self.ok("schema", "things", "create"))["body"]["properties"]
        self.assertEqual((body["tags"]["maxItems"], body["tags"]["uniqueItems"], body["tags"]["items"]["minLength"]),
                         (5, True, 1))
        self.assertEqual((body["contact"]["format"], body["sku"]["pattern"]), ("email", "^[A-Z]{3}-[0-9]+$"))
        self.assertEqual(body["website"]["examples"], ["https://acme.dev/lamp"])
        retrieve = json.loads(self.ok("schema", "things", "retrieve"))["parameters"]["properties"]["<THING_ID>"]
        self.assertEqual((retrieve["pattern"], retrieve["examples"]), ("^th_", ["th_123"]))
        server, send = self.mcp()
        tools = {t["name"]: t for t in send({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})["result"]["tools"]}
        self.assertEqual(tools["things_list"]["inputSchema"]["properties"]["limit"]["maximum"], 100)
        self.assertEqual(tools["things_create"]["inputSchema"]["properties"]["body"]["properties"]["price"]
                         ["exclusiveMinimum"], 0)
        server.stdin.close()
        self.assertEqual(server.wait(timeout=10), 0)

    def test_union_variants_declare_their_tag(self):
        body = json.loads(self.ok("schema", "things", "create"))["body"]["properties"]["delivery"]
        pickup, shipping = body["oneOf"]
        self.assertEqual((pickup["properties"]["method"], pickup["required"]),
                         ({"type": "string", "const": "pickup"}, ["method", "store"]))
        self.assertEqual(sorted(shipping["properties"]), ["address", "method", "note"])
        sent = json.loads(self.ok("things", "create", "--name", "Lamp", "--dry-run", "--delivery",
                                  '{"method": "pickup", "store": "Lyon", "note": "fragile"}'))
        self.assertEqual(sent["body"]["delivery"], {"method": "pickup", "store": "Lyon", "note": "fragile"})
        error = self.error(2, "things", "create", "--name", "Lamp", "--dry-run", "--delivery",
                           '{"method": "drone"}')
        self.assertIn("pickup", error["message"])
        labels = json.loads(self.ok("schema", "things", "create"))["body"]["properties"]["label"]["oneOf"]
        self.assertEqual([v["properties"]["kind"]["const"] for v in labels], ["text", "text", "image"])
        for label in [{"kind": "text", "text": "Fragile"}, {"kind": "text", "lines": ["Fragile", "Up"]}]:
            sent = json.loads(self.ok("things", "create", "--name", "Lamp", "--dry-run", "--label", json.dumps(label)))
            self.assertEqual(sent["body"]["label"], label)
        error = self.error(2, "things", "create", "--name", "Lamp", "--dry-run", "--label",
                           '{"kind": "text", "lines": "Fragile"}')
        self.assertEqual(error["errors"][0]["path"], "label.lines")
        server, send = self.mcp()
        tools = {t["name"]: t for t in send({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})["result"]["tools"]}
        variants = tools["things_create"]["inputSchema"]["properties"]["body"]["properties"]["delivery"]["anyOf"]
        self.assertEqual([v["properties"]["method"]["const"] for v in variants], ["pickup", "shipping"])
        server.stdin.close()
        self.assertEqual(server.wait(timeout=10), 0)

    def test_dry_run_prints_the_request_without_sending_it(self):
        request = json.loads(self.ok("things", "create", "--name", "Lamp", "--idempotency-key", "k1",
                                     "--dry-run", key=None))
        self.assertEqual((request["method"], request["url"], request["body"]),
                         ("POST", f"http://127.0.0.1:{self.port}/v1/things", {"name": "Lamp"}))
        self.assertEqual(request["headers"]["idempotency-key"], "k1")
        self.assertEqual(request["headers"]["authorization"], "Bearer <redacted>")
        listed = json.loads(self.ok("things", "list", "--status", "draft,active", "--dry-run"))
        self.assertEqual(listed["query"], {"status": ["draft", "active"]})
        self.assertNotIn("sk_test", json.dumps(listed))
        self.assertEqual(self.sent(), [])
        self.assertEqual(self.error(2, "things", "create", "--dry-run")["kind"], "usage")

    def test_output_is_compact_json_unless_asked_otherwise(self):
        self.assertEqual(self.ok("things", "retrieve", "th_1"),
                         '{"id":"th_1","name":"Widget","quantity":10,"status":"active","tags":[]}\n')
        self.assertTrue(self.ok("things", "retrieve", "th_1", "--pretty").startswith('{\n  "id"'))
        on_tty = self.tty("things", "list", "--max-items", "2", env={"ACME_FORMAT": "jsonl"})
        self.assertEqual([json.loads(line)["id"] for line in on_tty.splitlines()], ["th_1", "th_2"])
        self.assertEqual(json.loads(self.ok("things", "list", "--format", "json", "--jq", "length")), 5)
        self.assertEqual(self.ok("things", "list", "--jq", "[.id, .name] | @tsv", "--max-items", "1"),
                         "th_1\tWidget\n")

    def test_a_failing_filter_keeps_the_response(self):
        result = self.run_cli("things", "create", "--name", "Lamp", "--jq", ".name.first")
        self.assertEqual(result.returncode, 7, result.stderr)
        self.assertEqual(json.loads(result.stdout)["id"], "th_6")
        self.assertEqual(json.loads(result.stderr)["error"]["kind"], "output")

    def test_a_page_or_cursor_flag_fetches_one_page(self):
        lines = self.ok("things", "list", "--starting-after", "th_2", "--jq", ".id")
        self.assertEqual(lines, "th_3\nth_4\n")
        self.assertEqual(len(self.sent()), 1)
        self.assertEqual(self.ok("things", "list", "--max-items", "0"), "")
        self.assertEqual(len(self.sent()), 1)
        tty = self.tty("things", "list", "--format", "jsonl", "--jq", ".id")
        self.assertEqual(tty, "th_1\nth_2\nShowing the first 2 items: --all fetches every page, "
                              "--max-items N stops after N.\n")
        self.assertEqual(self.tty("things", "list", "--all", "--format", "jsonl", "--jq", ".id").count("th_"), 5)

    def test_event_streams_print_events_as_they_arrive_on_a_terminal_too(self):
        lines = self.tty("events", "list").splitlines()
        self.assertEqual([json.loads(line)["thing_id"] for line in lines], ["th_1", "th_2"])

    def test_rate_limits_say_when_to_retry(self):
        limited = self.error(3, "things", "retrieve", "th_429")
        self.assertEqual((limited["retryable"], limited["retry_after"]), (True, 7))
        self.assertFalse(self.error(3, "things", "retrieve", "th_404")["retryable"])

    def test_failed_requests_are_retried_when_safe(self):
        self.assertEqual(json.loads(self.ok("things", "retrieve", "th_flaky"))["id"], "th_1")
        self.assertEqual(len(self.sent()), 2)
        for args, env in [(["--no-retry"], None), (["--retries", "0"], None), ([], {"ACME_RETRIES": "0"})]:
            mark = len(self.requests())
            self.assertEqual(self.error(4, "things", "retrieve", "th_flaky", *args, env=env)["status"], 503)
            self.assertEqual(len(self.requests()) - mark, 1, args)
        mark = len(self.requests())
        self.assertEqual(self.error(4, "things", "create", "--name", "flaky")["status"], 503)
        self.assertEqual(len(self.requests()) - mark, 1, "a POST without an idempotency key is not retried")
        mark = len(self.requests())
        self.ok("things", "create", "--name", "flaky", "--idempotency-key", "k1")
        keys = [r["headers"]["idempotency-key"] for r in self.requests()[mark:]]
        self.assertEqual(keys, ["k1", "k1"])

    def test_debug_logs_each_attempt(self):
        stderr = self.run_cli("things", "retrieve", "th_flaky", "--debug").stderr.decode()
        attempts = [line for line in stderr.splitlines() if "/v1/things/th_flaky" in line]
        self.assertRegex(attempts[0], r"^debug: GET http://127\.0\.0\.1:\d+/v1/things/th_flaky: 503 in \d+ms$")
        self.assertRegex(attempts[1], r": 200 in \d+ms \(retry 1\)$")
        self.assertEqual(len(attempts), 2)

    def test_timeouts(self):
        timeout = self.error(5, "things", "retrieve", "th_slow", "--timeout", "0.2", "--no-retry")
        self.assertEqual((timeout["kind"], timeout["retryable"]), ("timeout", True))
        self.assertEqual(self.error(5, "things", "retrieve", "th_slow", "--no-retry",
                                    env={"ACME_TIMEOUT": "0.2"})["kind"], "timeout")
        invalid = self.error(2, "things", "list", env={"ACME_TIMEOUT": "soon"})
        self.assertEqual(invalid["message"], "ACME_TIMEOUT: expected a number of seconds, got `soon`")

    def mcp(self, *args, key="sk_test"):
        """An `acme mcp` server, and a function sending it a message and reading its answer."""
        server = subprocess.Popen([ACME, "mcp", *args], env=self.env(key), stdin=subprocess.PIPE,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, cwd=self.home.name)
        self.addCleanup(server.kill)
        self.addCleanup(server.stdout.close)
        self.addCleanup(server.stderr.close)

        def send(message):
            server.stdin.write(json.dumps(message) + "\n")
            server.stdin.flush()
            if "id" in message:
                response = json.loads(server.stdout.readline())
                self.assertEqual(response["id"], message["id"])
                return response
        return server, send

    def test_mcp_serves_every_operation_as_a_tool(self):
        server, send = self.mcp()
        calls = iter(range(10, 100))

        def call(tool, **arguments):
            message = {"jsonrpc": "2.0", "id": next(calls), "method": "tools/call",
                       "params": {"name": tool, "arguments": arguments}}
            result = send(message)["result"]
            texts = [c["text"] for c in result["content"] if c["type"] == "text"]
            return result, texts

        init = send({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "e2e", "version": "1"}}})
        self.assertEqual(init["result"]["protocolVersion"], "2025-11-25")
        self.assertEqual(init["result"]["serverInfo"]["name"], "acme")
        send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        tools = {t["name"]: t for t in send({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})["result"]["tools"]}
        self.assertIn("things_attachments_download", tools)
        self.assertNotIn("events_list", tools, "event streams never end: they are no tools")
        self.assertEqual(tools["things_list"]["annotations"]["readOnlyHint"], True)
        self.assertEqual(tools["things_delete"]["annotations"]["destructiveHint"], True)
        create = tools["things_create"]["inputSchema"]
        self.assertEqual(create["properties"]["body"]["required"], ["name"])
        self.assertEqual(create["properties"]["idempotency_key"]["type"], "string")
        self.assertEqual(create["required"], ["body"])

        result, texts = call("things_retrieve", thing_id="th_3")
        self.assertEqual(json.loads(texts[0])["name"], "Gizmo")
        result, texts = call("things_list", max_items=3, status=["draft", "active"])
        page = result["structuredContent"]
        self.assertEqual(json.loads(texts[0]), page)
        self.assertEqual([t["id"] for t in page["items"]], ["th_1", "th_2", "th_3"])
        self.assertEqual((page["has_more"], page["next"]), (True, {"starting_after": "th_3"}))
        self.assertIn("items", tools["things_list"]["outputSchema"]["properties"])
        result, texts = call("things_retrieve_batch", ids=["1", "3"], jq="[.[] | .id]")
        self.assertEqual(result["structuredContent"], {"jq": ["th_1", "th_3"]})
        result, texts = call("things_attachments_download", thing_id="th_1", attachment_id="at_1")
        blob = next(c["resource"] for c in result["content"] if c["type"] == "resource")
        self.assertEqual((blob["mimeType"], base64.b64decode(blob["blob"])),
                         ("application/pdf", b"%PDF-1.7 attachment\n"))
        sent = [(r["method"], r["path"]) for r in self.sent()]
        self.assertEqual(sent, [("GET", "/v1/things/th_3"), ("GET", "/v1/things"), ("GET", "/v1/things"),
                                ("GET", "/v1/things/batch/1,3"), ("GET", "/v1/things/th_1/attachments/at_1")])
        self.assertEqual(self.sent()[1]["query"]["status"], ["draft", "active"])

        mark = len(self.requests())
        result, texts = call("things_create", body={"name": "Lamp", "address": {"zip": "1"}})
        self.assertTrue(result["isError"])
        self.assertEqual(json.loads(texts[0])["error"]["errors"][0]["path"], "address.zip")
        result, texts = call("reports_retrieve", since="yesterday", kind="daily")
        self.assertEqual(json.loads(texts[0])["error"]["kind"], "usage")
        self.assertEqual(len(self.requests()), mark, "invalid calls send nothing")
        result, texts = call("things_retrieve", thing_id="th_404")
        self.assertEqual((result["isError"], json.loads(texts[0])["error"]["status"]), (True, 404))
        result, texts = call("things_delete", thing_id="th_1")
        self.assertEqual((result["isError"], self.requests()[-1]["method"]), (False, "DELETE"))

        meta = {"io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {}}
        discover = send({"jsonrpc": "2.0", "id": 3, "method": "server/discover", "params": {"_meta": meta}})
        self.assertEqual(discover["result"]["resultType"], "complete")
        self.assertEqual(discover["result"]["supportedVersions"][:2], ["2026-07-28", "2025-11-25"])
        server.stdin.close()
        self.assertEqual(server.wait(timeout=10), 0)

    def test_mcp_tools_can_be_restricted(self):
        server, send = self.mcp("--read-only", "--tools", "things_*", "--exclude-tools", "*_labels")
        listed = send({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})["result"]["tools"]
        self.assertEqual(sorted(t["name"] for t in listed),
                         ["things_attachments_download", "things_list",
                          "things_retrieve", "things_retrieve_batch"])
        server.stdin.close()
        self.assertEqual(server.wait(timeout=10), 0)

    def test_mcp_toolsets_and_overrides_come_from_perseid_toml(self):
        def tools(*args):
            server, send = self.mcp(*args)
            init = send({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "e2e", "version": "1"}}})
            listed = send({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})["result"]["tools"]
            server.stdin.close()
            self.assertEqual(server.wait(timeout=10), 0)
            return init["result"], {t["name"]: t for t in listed}
        init, default = tools()
        self.assertTrue(init["instructions"].startswith("Quantities are counts of items in stock."))
        self.assertIn("things_list", default)
        self.assertIn("reports_retrieve", default, "tools no toolset holds are served")
        self.assertNotIn("health_retrieve_ping", default)
        self.assertTrue(default["things_delete"]["description"].startswith("Delete a thing for good"))
        _, meta = tools("--toolsets", "meta")
        self.assertEqual(sorted(meta), ["auth_api_retrieve_whoami", "health_retrieve_ping", "reports_retrieve"])
        _, every = tools("--toolsets", "all")
        self.assertIn("health_retrieve_ping", every)
        error = self.error(2, "mcp", "--toolsets", "nope")
        self.assertEqual(error["message"], "--toolsets: no toolset `nope`: expected all or one of meta, things")

    def mcp_http(self, *args, key="sk_env"):
        """An `acme mcp --port 0` server, and a function posting it a message: status, headers, body."""
        server = subprocess.Popen([ACME, "mcp", "--port", "0", *args], env=self.env(key), stdin=subprocess.DEVNULL,
                                  stderr=subprocess.PIPE, text=True, cwd=self.home.name)
        self.addCleanup(server.wait)
        self.addCleanup(server.kill)
        self.addCleanup(server.stderr.close)
        url = server.stderr.readline().split("serving ")[1].strip()

        def post(message, key=None, headers=None):
            headers = {"content-type": "application/json", **(headers or {})}
            if key:
                headers["authorization"] = f"Bearer {key}"
            request = urllib.request.Request(url, json.dumps(message).encode(), headers)
            try:
                with urllib.request.urlopen(request, timeout=30) as response:
                    return response.status, response.headers, json.loads(response.read() or "null")
            except urllib.error.HTTPError as error:
                return error.code, error.headers, json.loads(error.read() or "null")
        return url, post

    def test_mcp_serves_http_with_the_key_of_each_request(self):
        url, post = self.mcp_http()
        with urllib.request.urlopen(url.replace("/mcp", "/health")) as response:
            self.assertEqual(json.load(response)["status"], "ok")
        status, _, listed = post({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}, key="sk_test")
        self.assertEqual(status, 200)
        self.assertIn("things_retrieve", [t["name"] for t in listed["result"]["tools"]])
        call = {"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": {"name": "things_retrieve", "arguments": {"thing_id": "th_3"}}}
        status, _, called = post(call, key="sk_test")
        self.assertEqual((status, called["result"]["structuredContent"]["name"]), (200, "Gizmo"))
        meta = {"io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {}}
        modern = {**call, "params": {**call["params"], "_meta": meta}}
        headers = {"mcp-protocol-version": "2026-07-28", "mcp-method": "tools/call", "mcp-name": "things_retrieve"}
        status, _, called = post(modern, key="sk_test", headers=headers)
        self.assertEqual((status, called["result"]["resultType"]), (200, "complete"))
        status, _, mismatch = post(modern, key="sk_test", headers={**headers, "mcp-name": "things_list"})
        self.assertEqual((status, mismatch["error"]["code"]), (400, -32020))
        status, _, rejected = post(call, key="sk_wrong")
        self.assertEqual(json.loads(rejected["result"]["content"][0]["text"])["error"]["status"], 401)
        mark = len(self.requests())
        status, headers, keyless = post(call)
        self.assertEqual((status, headers["www-authenticate"]), (401, 'Bearer realm="mcp"'))
        self.assertEqual(keyless["error"]["code"], -32600)
        status, _, _ = post(call, key="sk_test", headers={"origin": "https://evil.example"})
        self.assertEqual(status, 403)
        self.assertEqual(len(self.requests()), mark, "refused requests send nothing")

        _, post = self.mcp_http("--key-from-env", key="sk_test")
        status, _, called = post(call)
        self.assertEqual((status, called["result"]["isError"]), (200, False))

    def test_array_path_parameters_take_commas(self):
        things = json.loads(self.ok("things", "retrieve-batch", "1,3"))
        self.assertEqual([t["id"] for t in things], ["th_1", "th_3"])
        self.assertEqual(self.sent()[0]["path"], "/v1/things/batch/1,3")

    def test_dotted_properties_and_values_starting_with_a_dash(self):
        self.ok("things", "create", "--name", "Lamp", "--meta-source", "cli")
        self.assertEqual(json.loads(self.sent()[0]["body"]), {"name": "Lamp", "meta.source": "cli"})
        self.ok("things", "list", "--sort", "-name", "--max-items", "1")
        self.assertEqual(self.sent()[1]["query"]["sort"], ["-name"])

    def test_resources_named_like_schemas_lose_the_suffix(self):
        self.assertEqual(json.loads(self.ok("attachment", "retrieve", "at_9"))["id"], "at_9")

    def test_api_errors_map_to_exit_codes(self):
        not_found = self.error(3, "things", "retrieve", "th_404")
        self.assertEqual((not_found["status"], not_found["request_id"]), (404, "req_mock"))
        self.assertEqual(not_found["body"], {"message": "no thing th_404"})
        self.assertEqual(self.error(3, "things", "create", "--name", "taken")["status"], 409)
        self.assertEqual(self.error(4, "things", "retrieve", "th_500")["status"], 500)
        self.assertEqual(self.error(3, "things", "list", key="sk_bad")["status"], 401)
        self.assertEqual(self.error(6, "things", "list", key=None)["kind"], "config")

    def test_empty_responses_print_nothing(self):
        self.assertEqual(self.ok("things", "delete", "th_1"), "")
        self.assertEqual(self.sent()[0]["method"], "DELETE")

    def test_files_go_up_and_come_down(self):
        upload = Path(self.home.name) / "manual.pdf"
        upload.write_bytes(b"%PDF manual")
        out = self.ok("things", "attachments", "upload", "th_1", "--file", str(upload))
        self.assertEqual(json.loads(out)["size"], 11)
        self.assertEqual(self.sent()[0]["body"], "%PDF manual")
        self.assertEqual(json.loads(self.ok("things", "attachments", "upload", "th_1", "--file", "-",
                                            stdin=b"abc"))["size"], 3)
        target = Path(self.home.name) / "out.pdf"
        written = json.loads(self.ok("things", "attachments", "download", "th_1", "at_1", "-o", str(target)))
        self.assertEqual(written, {"path": str(target), "bytes": 20, "content_type": "application/pdf"})
        self.assertEqual(target.read_bytes(), b"%PDF-1.7 attachment\n")
        mark = len(self.sent())
        refused = self.error(2, "things", "attachments", "download", "th_1", "at_1")
        self.assertIn("-o FILE", refused["message"])
        self.assertEqual(len(self.sent()), mark)
        self.assertEqual(self.ok("things", "attachments", "download", "th_1", "at_1", "-o", "-"),
                         "%PDF-1.7 attachment\n")

    def test_multipart_bodies(self):
        photo = Path(self.home.name) / "photo.jpg"
        photo.write_bytes(b"jpeg")
        self.ok("things", "create-photo", "th_1", "--file", str(photo), "--caption", "At dawn")
        request = self.sent()[0]
        self.assertTrue(request["headers"]["content-type"].startswith("multipart/form-data"))
        self.assertIn('filename="photo.jpg"', request["body"])
        self.assertIn("At dawn", request["body"])

    def test_text_responses_and_operations_without_credentials(self):
        self.assertEqual(self.ok("health", "retrieve-ping", key=None), "pong")
        self.assertNotIn("authorization", self.sent()[0]["headers"])

    def test_event_streams_print_events_as_they_arrive(self):
        lines = self.ok("events", "list").splitlines()
        self.assertEqual([json.loads(line)["thing_id"] for line in lines], ["th_1", "th_2"])

    def test_required_query_parameters_of_any_type(self):
        report = json.loads(self.ok("reports", "retrieve", "--since", "2026-01-31", "--kind", "daily"))
        self.assertEqual(report, {"since": "2026-01-31", "kind": "daily", "count": 42})
        invalid = self.error(2, "reports", "retrieve", "--since", "yesterday", "--kind", "daily")
        self.assertTrue(invalid["message"].startswith("--since: invalid value `yesterday`"), invalid)

    def test_resources_named_like_builtins_are_renamed(self):
        caller = json.loads(self.ok("auth-api", "retrieve-whoami"))
        self.assertEqual(caller["email"], "jane@acme.dev")

    def test_oauth_urls_are_relative_to_the_base_url(self):
        with tempfile.TemporaryDirectory() as home:
            base = f"http://127.0.0.1:{self.port}/v1"
            pending = json.loads(self.ok("auth", "login", "--device", "--base-url", base, key=None, home=home))
            self.assertEqual((pending["verification_uri"], pending["user_code"], pending["expires_in"]),
                             ("https://acme.dev/activate", "ABCD-EFGH", 900))
            self.assertIn("next_step", pending)
            device, token = self.sent()
            self.assertEqual((device["path"], token["path"]), ("/oauth/device", "/oauth/token"))
            self.assertEqual(parse_qs(device["body"])["scope"], ["things:read"])
            self.assertEqual(json.loads(self.ok("things", "retrieve", "th_1", key=None, home=home))["id"], "th_1")
            self.ok("auth", "logout", key=None, home=home)
            revoke = self.sent()[-1]
            self.assertEqual((revoke["path"], parse_qs(revoke["body"])["token"]), ("/oauth/revoke", ["rt_1"]))

    def test_auth_status_and_token_for_scripts(self):
        with tempfile.TemporaryDirectory() as home:
            none = self.run_cli("auth", "status", key=None, home=home)
            self.assertEqual(none.returncode, 6, none.stderr)
            self.assertFalse(json.loads(none.stdout)["logged_in"])
            self.assertEqual(json.loads(none.stderr)["error"]["kind"], "config")
            self.ok("auth", "login", "--api-key-stdin", key=None, home=home, stdin=b"sk_test\n")
            status = json.loads(self.ok("auth", "status", key=None, home=home))
            self.assertEqual((status["logged_in"], status["method"], status["storage"]), (True, "api_key", "file"))
            self.assertEqual(status["base_url"], f"http://127.0.0.1:{self.port}")
            self.assertNotIn("sk_test", json.dumps(status))
            self.assertEqual(self.ok("auth", "token", key=None, home=home), "sk_test\n")
            self.assertEqual(self.ok("auth", "token", key="sk_env", home=home), "sk_env\n")

    def test_logins_use_the_keyring_when_there_is_one(self):
        with tempfile.TemporaryDirectory() as home:
            login = self.run_cli("auth", "login", "--api-key-stdin", key=None, home=home, stdin=b"sk_test\n",
                                 env={"ACME_NO_KEYRING": None})
            self.assertEqual(login.returncode, 0, login.stderr)
            in_keyring = "its secrets in the OS keyring" in login.stderr.decode()
            if not in_keyring:
                self.assertIn("warning: the OS keyring is unavailable", login.stderr.decode())
            status = json.loads(self.ok("auth", "status", key=None, home=home, env={"ACME_NO_KEYRING": None}))
            self.assertEqual(status["storage"], "keyring" if in_keyring else "file")
            self.assertEqual(json.loads(self.ok("things", "retrieve", "th_1", key=None, home=home))["id"], "th_1")
            self.ok("auth", "logout", key=None, home=home)

    def test_browser_login_survives_an_idle_connection(self):
        with tempfile.TemporaryDirectory() as home:
            cli = subprocess.Popen([ACME, "auth", "login", "--web"], env=self.env(None, home),
                                   stdin=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
            line = ""
            while "/authorize?" not in line:
                line = cli.stderr.readline()
                self.assertTrue(line, "no authorization URL")
            query = parse_qs(urlparse(line.strip()).query)
            port = urlparse(query["redirect_uri"][0]).port
            with socket.create_connection(("127.0.0.1", port)), \
                    socket.create_connection(("127.0.0.1", port), timeout=10) as browser:
                browser.sendall(f"GET /callback?code=code_1&state={query['state'][0]} HTTP/1.1\r\n\r\n".encode())
                self.assertTrue(browser.recv(100).startswith(b"HTTP/1.1 200 OK"))
            self.assertEqual(cli.wait(timeout=30), 0, cli.stderr.read())
            cli.stderr.close()
            self.assertEqual(self.sent()[-1]["path"], "/oauth/token")


if __name__ == "__main__":
    unittest.main()

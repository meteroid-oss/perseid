"""The Acme API of tests/acme/openapi.json, in memory: `python3 mock.py PORT`.

It records every request, which `GET /__requests` returns, so tests check what the CLI sent.
"""

import json
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

THINGS = [
    {"id": f"th_{i}", "name": name, "quantity": i * 10, "status": "active", "tags": []}
    for i, name in enumerate(["Widget", "Gadget", "Gizmo", "Doohickey", "Sprocket"], 1)
]
REQUESTS = []


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):
        pass

    def send(self, status, body=None, content_type="application/json", headers=()):
        data = b"" if body is None else body if isinstance(body, bytes) else json.dumps(body).encode()
        self.send_response(status)
        for name, value in headers:
            self.send_header(name, value)
        if body is not None:
            self.send_header("content-type", content_type)
        self.send_header("x-request-id", "req_mock")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def handle_any(self):
        url = urlparse(self.path)
        length = int(self.headers.get("content-length") or 0)
        raw = self.rfile.read(length) if length else b""
        if url.path == "/__requests":
            return self.send(200, REQUESTS)
        query = parse_qs(url.query)
        REQUESTS.append({
            "method": self.command,
            "path": url.path,
            "query": query,
            "headers": {k.lower(): v for k, v in self.headers.items()},
            "body": raw.decode("latin-1"),
        })
        if url.path.startswith("/oauth/"):
            return self.oauth(url.path, parse_qs(raw.decode()))
        if url.path == "/v1/ping":
            return self.send(200, b"pong", "text/plain")
        if self.headers.get("authorization") != "Bearer sk_test":
            return self.send(401, {"message": "invalid API key"})
        route = (self.command, url.path.split("/")[2:])
        match route:
            case ("GET", ["things", "th_flaky"]) | ("POST", ["things"]) if self.flaky(raw):
                return self.send(503, {"message": "try again"}, headers=[("retry-after-ms", "1")])
            case ("GET", ["things"]):
                after = query.get("starting_after", [None])[0]
                limit = int(query.get("limit", ["2"])[0])
                start = next((i + 1 for i, t in enumerate(THINGS) if t["id"] == after), 0)
                page = THINGS[start:start + limit]
                return self.send(200, {"data": page, "has_more": start + limit < len(THINGS)})
            case ("POST", ["things"]):
                body = json.loads(raw)
                if body.get("name") == "taken":
                    return self.send(409, {"message": "a thing named `taken` exists"})
                thing = {"id": "th_6", "quantity": 0, "status": "draft", "tags": []} | body
                return self.send(201, thing)
            case ("GET", ["things", "th_404"]):
                return self.send(404, {"message": "no thing th_404"})
            case ("GET", ["things", "th_slow"]):
                time.sleep(2)
                return self.send(200, THINGS[0])
            case ("GET", ["things", "th_429"]):
                limited = [("retry-after", "7"), ("retry-after-ms", "1")]
                return self.send(429, {"message": "slow down"}, headers=limited)
            case ("GET", ["things", "batch", ids]):
                wanted = ids.split(",")
                return self.send(200, [t for t in THINGS if t["id"][3:] in wanted])
            case ("GET", ["labels"]):
                return self.send(200, ["new", "sale"])
            case ("GET", ["attachments", attachment_id]):
                return self.send(200, {"id": attachment_id, "size": 3})
            case ("GET", ["things", "th_500"]):
                return self.send(500, {"message": "the catalog is down"})
            case ("GET", ["things", thing_id]):
                return self.send(200, next((t for t in THINGS if t["id"] == thing_id), THINGS[0]))
            case ("DELETE", ["things", _]):
                return self.send(204)
            case ("POST", ["things", _, "attachments"]):
                return self.send(201, {"id": "at_1", "size": len(raw)})
            case ("GET", ["things", _, "attachments", _]):
                return self.send(200, b"%PDF-1.7 attachment\n", "application/pdf")
            case ("POST", ["things", _, "photos"]):
                return self.send(201, {"id": "ph_1", "size": len(raw), "name": "photo"})
            case ("GET", ["whoami"]):
                return self.send(200, {"id": "usr_1", "email": "jane@acme.dev"})
            case ("GET", ["events"]):
                events = "".join(
                    f"event: thing.updated\ndata: {json.dumps({'type': 'thing.updated', 'thing_id': t['id']})}\n\n"
                    for t in THINGS[:2]
                )
                return self.send(200, events.encode(), "text/event-stream")
            case ("GET", ["reports"]):
                return self.send(200, {"since": query["since"][0], "kind": query["kind"][0], "count": 42})
        return self.send(404, {"message": f"no route {self.command} {url.path}"})

    def flaky(self, raw):
        """Whether to fail `th_flaky`, or a thing named `flaky`: on the first attempt only."""
        flaky = self.path.endswith("/th_flaky") or (raw and json.loads(raw).get("name") == "flaky")
        retried = any(k.lower().endswith("-retry-count") for k in self.headers.keys())
        return flaky and not retried

    def oauth(self, path, form):
        """The authorization server, at relative URLs of the spec: every login gets `sk_test`."""
        if path == "/oauth/device":
            return self.send(200, {"device_code": "dc_1", "user_code": "ABCD-EFGH",
                                   "verification_uri": "https://acme.dev/activate", "interval": 0})
        if path == "/oauth/token" and form.get("code", form.get("device_code")) in (["code_1"], ["dc_1"]):
            return self.send(200, {"access_token": "sk_test", "token_type": "Bearer",
                                   "expires_in": 3600, "refresh_token": "rt_1"})
        if path == "/oauth/revoke":
            return self.send(200, {})
        return self.send(400, {"error": "invalid_grant"})

    do_GET = do_POST = do_DELETE = do_PUT = do_PATCH = handle_any


if __name__ == "__main__":
    ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), Handler).serve_forever()

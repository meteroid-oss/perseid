"""Mock of tests/fixtures/features.yaml for the generated SDK smoke tests.

Prints its base URL on the first line of stdout, then serves until killed.
"""

import json
from email.message import Message
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

WIDGETS = {None: (["w1", "w2"], "c2"), "c2": (["w3"], None)}
EVENTS = {None: (["e1", "e2"], True), "e2": (["e3"], False)}
GADGETS = {0: ["g1", "g2"], 1: ["g3"]}
RECORDS = {0: ["r1", "r2"], 2: ["r3"]}
STREAM = (
    ": keep-alive\n\n"
    "event: greeting\nid: 1\ndata: TOPIC\n\n"
    "data: line1\ndata: line2\n\n"
    "id: 3\r\nretry: 1500\r\ndata: {\"n\": 3}\r\n\r\n"
)


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):
        pass

    def reply(self, status, body, content_type="application/json"):
        data = body if isinstance(body, bytes) else json.dumps(body).encode()
        self.send_response(status)
        self.send_header("content-type", content_type)
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def auth(self):
        query = parse_qs(urlparse(self.path).query)
        return "|".join(
            [
                self.headers.get("authorization", ""),
                self.headers.get("x-api-key", ""),
                query.get("api_key", [""])[0],
            ]
        )

    def body(self):
        if self.headers.get("transfer-encoding", "").lower() == "chunked":
            data = b""
            while True:
                size = int(self.rfile.readline().strip(), 16)
                chunk = self.rfile.read(size + 2)[:size]
                if not size:
                    return data
                data += chunk
        return self.rfile.read(int(self.headers.get("content-length", "0")))

    def multipart(self):
        boundary = self.headers["content-type"].split("boundary=")[1].strip('"').encode()
        parts = []
        for raw in self.body().split(b"--" + boundary)[1:-1]:
            head, _, content = raw[2:-2].partition(b"\r\n\r\n")
            lines = [line.decode().split(": ", 1) for line in head.split(b"\r\n") if line]
            headers = {key.lower(): value for key, value in lines}
            disposition = Message()
            disposition["content-disposition"] = headers["content-disposition"]
            name = disposition.get_param("name", header="content-disposition")
            filename = disposition.get_param("filename", "", header="content-disposition")
            kind = headers.get("content-type", "")
            parts.append(f"{name}={filename}:{kind}:{content.decode()}")
        return ";".join(parts)

    def handle_any(self):
        url = urlparse(self.path)
        query = {k: v[0] for k, v in parse_qs(url.query).items()}
        path = url.path
        if path == "/events/stream":
            data = STREAM.replace("TOPIC", query.get("topic", "")).encode()
            self.send_response(200)
            self.send_header("content-type", "text/event-stream; charset=utf-8")
            self.send_header("content-length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
            return
        if path == "/files":
            return self.reply(200, {"status": self.multipart()})
        if path == "/files/f1/content":
            body = self.body().decode()
            return self.reply(200, {"status": f"{self.headers['content-type']}:{body}"})
        if path in ("/health", "/session", "/machine"):
            return self.reply(200, {"status": self.auth()})
        if path == "/widgets":
            if self.auth() == "||":
                return self.reply(401, {"error": "unauthorized"})
            ids, cursor = WIDGETS[query.get("cursor")]
            return self.reply(200, {"data": [{"id": i, "name": i} for i in ids], "next_cursor": cursor})
        if path == "/widgets/w1/events":
            if query.get("kind") != "created":
                return self.reply(400, {"error": "kind"})
            ids, more = EVENTS[query.get("starting_after")]
            return self.reply(200, {"data": [{"id": i, "kind": "created"} for i in ids], "has_more": more})
        if path == "/gadgets":
            page = int(query.get("page", "0"))
            return self.reply(200, {"items": [{"id": i} for i in GADGETS[page]], "meta": {"page": page, "total_pages": 2}})
        if path == "/records":
            if query.get("api_key") != "tok":
                return self.reply(401, {"error": "api_key"})
            ids = RECORDS[int(query.get("offset", "0"))]
            return self.reply(200, {"data": [{"id": i} for i in ids], "total": 3})
        return self.reply(404, {"error": path})

    do_GET = do_POST = do_PUT = handle_any


if __name__ == "__main__":
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    print(f"http://127.0.0.1:{server.server_address[1]}", flush=True)
    server.serve_forever()

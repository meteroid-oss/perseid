"""The OAuth2 client credentials flow of the SDK generated from tests/fixtures/oauth.yaml, on an
in-memory transport that plays the token endpoint and the API."""

import asyncio
import base64
import os
import unittest
import urllib.parse

import httpx

from _support import generate_fixture, import_package

vault = import_package(generate_fixture("oauth.yaml", "Vault"))

os.environ.pop("VAULT_API_KEY", None)
os.environ.pop("VAULT_BASE_URL", None)
os.environ.pop("VAULT_CLIENT_ID", None)
os.environ.pop("VAULT_CLIENT_SECRET", None)
vault.api.common.INITIAL_RETRY_DELAY = 0.0

BASE = "https://vault.test/v1"
TOKEN_URL = f"{BASE}/oauth/token"


def basic(client_id: str, secret: str) -> str:
    return f"Basic {base64.b64encode(f'{client_id}:{secret}'.encode()).decode()}"


class Server:
    """Issues the access tokens `at-1`, `at-2`... and rejects those in `revoked` with a 401."""

    def __init__(self, expires_in: float = 3600, revoked: tuple[str, ...] = (), scripted=()) -> None:
        self.calls: list[httpx.Request] = []
        self.issued = 0
        self.expires_in = expires_in
        self.revoked = revoked
        self.scripted = list(scripted)

    @property
    def token_calls(self) -> list[httpx.Request]:
        return [call for call in self.calls if str(call.url) == TOKEN_URL]

    def __call__(self, request: httpx.Request) -> httpx.Response:
        self.calls.append(request)
        if str(request.url) == TOKEN_URL:
            if self.scripted:
                return self.scripted.pop(0)
            self.issued += 1
            return httpx.Response(
                200,
                json={
                    "access_token": f"at-{self.issued}",
                    "token_type": "Bearer",
                    "expires_in": self.expires_in,
                },
            )
        token = request.headers.get("authorization", "").removeprefix("Bearer ")
        if token in self.revoked:
            return httpx.Response(401, json={"message": "revoked"})
        return httpx.Response(200, json={"status": token or "anonymous"})


def client(server: Server, **options):
    options = {"client_id": "id", "client_secret": "secret", "max_retries": 0, "base_url": BASE, **options}
    http = httpx.Client(transport=httpx.MockTransport(server))
    return vault.Vault(http_client=http, **options)


def async_client(server: Server, **options):
    options = {"client_id": "id", "client_secret": "secret", "max_retries": 0, "base_url": BASE, **options}
    http = httpx.AsyncClient(transport=httpx.MockTransport(server))
    return vault.AsyncVault(http_client=http, **options)


class OAuthTest(unittest.TestCase):
    def test_a_token_is_fetched_on_first_use_and_kept(self) -> None:
        server = Server()
        with client(server) as api:
            self.assertEqual(api.account.retrieve_machine().status, "at-1")
            self.assertEqual(api.account.retrieve_machine().status, "at-1")
        self.assertEqual(len(server.token_calls), 1)
        token = server.token_calls[0]
        self.assertEqual(token.method, "POST")
        self.assertEqual(token.headers["content-type"], "application/x-www-form-urlencoded")
        self.assertEqual(token.headers["authorization"], basic("id", "secret"))
        self.assertEqual(
            urllib.parse.parse_qsl(token.content.decode()),
            [("grant_type", "client_credentials"), ("scope", "secrets.read secrets.write")],
        )
        self.assertEqual(server.calls[1].headers["authorization"], "Bearer at-1")

    def test_schemes_sharing_a_token_url_keep_a_token_per_scope(self) -> None:
        server = Server()
        with client(server) as api:
            self.assertEqual(api.account.retrieve_machine().status, "at-1")
            self.assertEqual(api.account.retrieve_admin().status, "at-2")
            self.assertEqual(api.account.retrieve_machine().status, "at-1")
            self.assertEqual(api.account.retrieve_admin().status, "at-2")
        scopes = [dict(urllib.parse.parse_qsl(call.content.decode()))["scope"] for call in server.token_calls]
        self.assertEqual(scopes, ["secrets.read secrets.write", "secrets.admin"])

    def test_an_absolute_path_token_url_resolves_against_the_origin(self) -> None:
        resolve = vault.api.common.token_url
        self.assertEqual(resolve(BASE, "/oauth/token"), "https://vault.test/oauth/token")
        self.assertEqual(resolve(BASE, "oauth/token"), TOKEN_URL)
        self.assertEqual(resolve(BASE, "https://auth.test/t"), "https://auth.test/t")

    def test_public_operations_send_no_credentials_and_fetch_no_token(self) -> None:
        server = Server()
        with client(server) as api:
            self.assertEqual(api.account.check_health().status, "anonymous")
            self.assertEqual(server.token_calls, [])
            self.assertEqual(api.account.create_session().status, "at-1")

    def test_credentials_are_form_encoded_before_the_basic_header_or_sent_in_the_body(self) -> None:
        server = Server()
        with client(server, client_id="a b", client_secret="p@ss:word") as api:
            api.account.retrieve_machine()
        self.assertEqual(server.token_calls[0].headers["authorization"], basic("a+b", "p%40ss%3Aword"))

        server = Server()
        with client(server, oauth_client_auth="body") as api:
            api.account.retrieve_machine()
        token = server.token_calls[0]
        self.assertNotIn("authorization", token.headers)
        form = dict(urllib.parse.parse_qsl(token.content.decode()))
        self.assertEqual((form["client_id"], form["client_secret"]), ("id", "secret"))

    def test_an_expired_token_is_renewed(self) -> None:
        server = Server(expires_in=0)
        with client(server) as api:
            self.assertEqual(api.account.retrieve_machine().status, "at-1")
            self.assertEqual(api.account.retrieve_machine().status, "at-2")
        self.assertEqual(len(server.token_calls), 2)

    def test_a_token_the_api_rejects_is_replaced_once(self) -> None:
        server = Server(revoked=("at-1",))
        with client(server) as api:
            self.assertEqual(api.account.retrieve_machine().status, "at-2")
            self.assertEqual(len(server.token_calls), 2)
            self.assertEqual(len(server.calls) - len(server.token_calls), 2)
            self.assertEqual(api.account.retrieve_machine().status, "at-2")
            self.assertEqual(len(server.token_calls), 2)

        server = Server(revoked=("at-1", "at-2", "at-3"))
        with client(server) as api, self.assertRaises(vault.AuthenticationError):
            api.account.retrieve_machine()
        self.assertEqual(len(server.token_calls), 2, "a second 401 is the caller's")

    def test_the_token_request_is_retried_like_any_request(self) -> None:
        server = Server(scripted=[httpx.Response(503, json={"error": "busy"})])
        with client(server, max_retries=1) as api:
            self.assertEqual(api.account.retrieve_machine().status, "at-1")
        self.assertEqual(len(server.token_calls), 2)

        server = Server(scripted=[httpx.Response(401, json={"error": "invalid_client"})])
        with client(server) as api, self.assertRaises(vault.AuthenticationError) as caught:
            api.account.retrieve_machine()
        self.assertEqual(caught.exception.body["error"], "invalid_client")
        self.assertEqual(len(server.calls), 1, "the API is not called without a token")

        server = Server(scripted=[httpx.Response(200, json={"token_type": "Bearer"})])
        with client(server) as api, self.assertRaises(vault.APIResponseValidationError):
            api.account.retrieve_machine()

    def test_a_relative_token_url_is_resolved_against_the_base_url(self) -> None:
        server = Server()

        def forward(request: httpx.Request) -> httpx.Response:
            url = str(request.url).replace("https://other.test/api", BASE)
            return server(
                httpx.Request(request.method, url, headers=request.headers, content=request.content)
            )

        http = httpx.Client(transport=httpx.MockTransport(forward))
        with vault.Vault(
            http_client=http, client_id="id", client_secret="secret", base_url="https://other.test/api/"
        ) as api:
            api.account.retrieve_machine()
        self.assertEqual(str(server.calls[0].url), TOKEN_URL)

    def test_a_token_provider_or_the_api_key_wins(self) -> None:
        server = Server()
        with client(server, token_provider=lambda: "mine") as api:
            self.assertEqual(api.account.retrieve_machine().status, "mine")
        with client(server, api_key="static") as api:
            self.assertEqual(api.account.retrieve_machine().status, "static")
        self.assertEqual(server.token_calls, [])

    def test_the_credentials_default_to_the_environment(self) -> None:
        os.environ["VAULT_CLIENT_ID"], os.environ["VAULT_CLIENT_SECRET"] = "env-id", "env-secret"
        try:
            server = Server()
            http = httpx.Client(transport=httpx.MockTransport(server))
            with vault.Vault(http_client=http, base_url=BASE, max_retries=0) as api:
                self.assertEqual(api.account.retrieve_machine().status, "at-1")
            self.assertEqual(
                server.token_calls[0].headers["authorization"], basic("env-id", "env-secret")
            )
        finally:
            del os.environ["VAULT_CLIENT_ID"], os.environ["VAULT_CLIENT_SECRET"]


class AsyncOAuthTest(unittest.TestCase):
    def test_concurrent_calls_share_one_token_request(self) -> None:
        async def run() -> None:
            server = Server()
            async with async_client(server) as api:
                results = await asyncio.gather(*(api.account.retrieve_machine() for _ in range(3)))
            self.assertEqual([r.status for r in results], ["at-1"] * 3)
            self.assertEqual(len(server.token_calls), 1)

        asyncio.run(run())

    def test_a_rejected_token_is_replaced(self) -> None:
        async def run() -> None:
            server = Server(revoked=("at-1",))
            async with async_client(server) as api:
                self.assertEqual((await api.account.retrieve_machine()).status, "at-2")
            self.assertEqual(len(server.token_calls), 2)

        asyncio.run(run())


if __name__ == "__main__":
    unittest.main()

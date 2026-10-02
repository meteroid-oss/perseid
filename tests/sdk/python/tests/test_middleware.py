import asyncio
import unittest

import httpx

from petstore import AsyncPetstore, Petstore

PET = {"id": "1", "name": "Rex", "created_at": "2024-01-01T00:00:00Z"}


def make_cache():
    store = {}

    def cache(request, next):
        if request.method != "GET":
            return next(request)
        key = str(request.url)
        if key not in store:
            response = next(request)
            if not response.is_success:
                return response
            store[key] = response.content
        return httpx.Response(200, content=store[key])

    return cache


def make_async_cache():
    store = {}

    async def cache(request, next):
        key = str(request.url)
        if key not in store:
            store[key] = (await next(request)).content
        return httpx.Response(200, content=store[key])

    return cache


class MiddlewareTest(unittest.TestCase):
    def origin(self):
        self.seen = []

        def origin(request, next):
            self.seen.append(request)
            return httpx.Response(200, json=PET)

        return origin

    def test_cache_answers_repeated_gets_without_reaching_the_origin(self):
        with Petstore(api_key="token", middleware=[make_cache(), self.origin()]) as client:
            for _ in range(3):
                self.assertEqual(client.pets.retrieve("1").name, "Rex")
            self.assertEqual(len(self.seen), 1)
            client.pets.retrieve("2")
            self.assertEqual(len(self.seen), 2)

    def test_middleware_can_change_the_request(self):
        def tag(request, next):
            request.headers["x-tag"] = "yes"
            return next(request)

        with Petstore(api_key="token", middleware=[tag, self.origin()]) as client:
            client.pets.retrieve("1")
        self.assertEqual(self.seen[0].headers["x-tag"], "yes")
        self.assertEqual(self.seen[0].headers["authorization"], "Bearer token")

    def test_async_client_uses_async_middleware(self):
        calls = []

        async def origin(request, next):
            calls.append(request)
            return httpx.Response(200, json=PET)

        async def run():
            async with AsyncPetstore(api_key="token", middleware=[make_async_cache(), origin]) as client:
                for _ in range(3):
                    self.assertEqual((await client.pets.retrieve("1")).name, "Rex")

        asyncio.run(run())
        self.assertEqual(len(calls), 1)


if __name__ == "__main__":
    unittest.main()

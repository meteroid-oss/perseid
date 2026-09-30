"""Customize API error decoding here without changing generated files."""

from .serialization import @@CLIENT_NAME@@Error, ModelParseError

__all__ = [
    "@@CLIENT_NAME@@Error",
    "ApiException",
    "ModelParseError",
    "NetworkException",
    "ResponseDecodeError",
]


class NetworkException(@@CLIENT_NAME@@Error):
    """The request got no response: connection failure or timeout, after retries."""


class ApiException(@@CLIENT_NAME@@Error):
    """The API answered with a non-2xx status."""

    def __init__(self, status_code: int, raw_body: bytes) -> None:
        self.status_code = status_code
        self.raw_body = raw_body
        super().__init__(f"API error {status_code}: {raw_body!r}")

    @classmethod
    def from_response(cls, status_code: int, raw_body: bytes) -> "ApiException":
        return cls(status_code, raw_body)


class ResponseDecodeError(@@CLIENT_NAME@@Error, ValueError):
    """A 2xx body was not valid JSON or did not match its model."""

    def __init__(self, status_code: int, raw_body: bytes, reason: str) -> None:
        self.status_code = status_code
        self.raw_body = raw_body
        super().__init__(f"Could not decode response ({status_code}): {reason}")

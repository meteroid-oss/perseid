"""Every error this SDK raises derives from `@@CLIENT_NAME@@Error`.

* `APIStatusError` (and a subclass per common status): the API answered with
  an error status. `body` is the error response decoded into its schema.
* `APIConnectionError`: no response, after retries; `APITimeoutError` when the
  request timed out.
* `APIResponseValidationError`: a successful response that does not decode.
"""

from ._exceptions import (
    APIConnectionError,
    APIError,
    APIResponseValidationError,
    APITimeoutError,
    NetworkException,
    ResponseDecodeError,
)
from .api._errors import (
    APIStatusError,
    ApiException,
    ApiStatusError,
    AuthenticationError,
    BadRequestError,
    ConflictError,
    InternalServerError,
    NotFoundError,
    PermissionDeniedError,
    RateLimitError,
    UnprocessableEntityError,
)
from .serialization import @@CLIENT_NAME@@Error, ModelParseError

__all__ = [
    "@@CLIENT_NAME@@Error",
    "APIConnectionError",
    "APIError",
    "APIResponseValidationError",
    "APIStatusError",
    "APITimeoutError",
    "ApiException",
    "ApiStatusError",
    "AuthenticationError",
    "BadRequestError",
    "ConflictError",
    "InternalServerError",
    "ModelParseError",
    "NetworkException",
    "NotFoundError",
    "PermissionDeniedError",
    "RateLimitError",
    "ResponseDecodeError",
    "UnprocessableEntityError",
]

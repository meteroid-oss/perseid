//! Errors returned by the client.
use std::{borrow::Cow, fmt};

use bytes::Bytes;
use http::{HeaderMap, StatusCode};

use crate::request::Failure;

pub use crate::api::middleware::BoxError;

/// A `Result` failing with the client's [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Everything a call can fail with.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The API answered with a non-2xx status, after any retries.
    Api(Box<ApiError>),
    /// No response arrived within the configured timeout.
    Timeout,
    /// Connecting, sending the request or reading the response failed.
    Connection(BoxError),
    /// The response body does not match the expected type.
    Decode(BoxError),
    /// The request or the client could not be built, e.g. an invalid header value or a
    /// missing base URL.
    Request(BoxError),
}

/// A non-2xx response.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ApiError {
    /// The HTTP status.
    pub status: StatusCode,
    /// The response headers.
    pub headers: HeaderMap,
    /// The raw body.
    pub body: Bytes,
}

/// What an API error status means.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ApiErrorKind {
    /// 400
    BadRequest,
    /// 401
    Unauthorized,
    /// 403
    PermissionDenied,
    /// 404
    NotFound,
    /// 409
    Conflict,
    /// 422
    UnprocessableEntity,
    /// 429
    RateLimited,
    /// 5xx
    InternalServer,
    /// Any other status.
    Other,
}

impl ApiError {
    /// What the status means: not found, rate limited...
    #[must_use]
    pub fn kind(&self) -> ApiErrorKind {
        match self.status.as_u16() {
            400 => ApiErrorKind::BadRequest,
            401 => ApiErrorKind::Unauthorized,
            403 => ApiErrorKind::PermissionDenied,
            404 => ApiErrorKind::NotFound,
            409 => ApiErrorKind::Conflict,
            422 => ApiErrorKind::UnprocessableEntity,
            429 => ApiErrorKind::RateLimited,
            500..=599 => ApiErrorKind::InternalServer,
            _ => ApiErrorKind::Other,
        }
    }

    /// The body as text, for logging.
    #[must_use]
    pub fn text(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.body)
    }

    /// The body decoded as `T`, e.g. the error schema the operation documents.
    ///
    /// # Errors
    ///
    /// Fails when the body is not a `T`.
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> serde_json::Result<T> {
        serde_json::from_slice(&self.body)
    }

    /// The body decoded as the error schema most operations document (any JSON when the API
    /// documents none), if it is one.
    #[must_use]
    pub fn payload(&self) -> Option<crate::api::ErrorBody> {
        self.json().ok()
    }

    /// The message of a JSON body: its `error.message`, else its `message` or `detail` string.
    #[must_use]
    pub fn message(&self) -> Option<String> {
        let body: serde_json::Value = serde_json::from_slice(&self.body).ok()?;
        let text = |value: Option<&serde_json::Value>| Some(value?.as_str()?.to_owned());
        text(body.pointer("/error/message"))
            .or_else(|| text(body.get("message")))
            .or_else(|| text(body.get("detail")))
    }

    /// The `x-request-id` (or `request-id`) response header, to quote when reporting an issue.
    #[must_use]
    pub fn request_id(&self) -> Option<&str> {
        ["x-request-id", "request-id"]
            .iter()
            .find_map(|name| self.headers.get(*name)?.to_str().ok())
    }
}

impl Error {
    // The runtime builds errors through these two functions only.
    pub(crate) fn generic(failure: Failure) -> Self {
        match failure {
            Failure::Timeout => Self::Timeout,
            Failure::Transport(error) => Self::Connection(error),
            Failure::Decode(error) => Self::Decode(error),
            Failure::Request(error) => Self::Request(error),
        }
    }

    pub(crate) fn from_response(status: StatusCode, headers: HeaderMap, body: Bytes) -> Self {
        Self::Api(Box::new(ApiError {
            status,
            headers,
            body,
        }))
    }

    /// Whether no response arrived within the timeout.
    #[must_use]
    pub fn is_timeout(&self) -> bool {
        matches!(self, Self::Timeout)
    }

    /// Whether connecting, sending the request or reading the response failed.
    #[must_use]
    pub fn is_connection(&self) -> bool {
        matches!(self, Self::Connection(_))
    }

    /// The HTTP status of an API error.
    #[must_use]
    pub fn status(&self) -> Option<StatusCode> {
        self.api().map(|error| error.status)
    }

    /// What the status of an API error means.
    #[must_use]
    pub fn kind(&self) -> Option<ApiErrorKind> {
        self.api().map(ApiError::kind)
    }

    /// The response of an API error.
    #[must_use]
    pub fn api(&self) -> Option<&ApiError> {
        match self {
            Self::Api(error) => Some(error),
            _ => None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Api(error) => match error.message() {
                Some(message) => write!(f, "API error ({}): {message}", error.status),
                None => write!(f, "API error ({}): {}", error.status, error.text()),
            },
            Self::Timeout => f.write_str("request timed out"),
            Self::Connection(error) => write!(f, "connection error: {error}"),
            Self::Decode(error) => write!(f, "unexpected response body: {error}"),
            Self::Request(error) => write!(f, "invalid request: {error}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Connection(error) | Self::Decode(error) | Self::Request(error) => Some(&**error),
            Self::Api(_) | Self::Timeout => None,
        }
    }
}

//! Errors returned by the client.
use std::{borrow::Cow, fmt};

use bytes::Bytes;
use http1::{HeaderMap, StatusCode};

use crate::request::Failure;

pub use crate::api::middleware::BoxError;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The API answered with a non-2xx status, after any retries.
    Api(Box<ApiError>),
    /// No response arrived within the configured timeout.
    Timeout,
    /// Connecting, sending the request or reading the response failed.
    Transport(BoxError),
    /// The response body does not match the expected type.
    Decode(BoxError),
    /// The request could not be built, e.g. an invalid header value.
    Request(BoxError),
}

/// A non-2xx response.
#[derive(Clone, Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Bytes,
}

impl ApiError {
    /// The body as text, for logging.
    pub fn text(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.body)
    }

    /// The body decoded as `T`, e.g. the API's error schema.
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> serde_json::Result<T> {
        serde_json::from_slice(&self.body)
    }

    /// The body decoded as the error schema most operations document, if it is one.
    pub fn payload(&self) -> Option<crate::api::ErrorBody> {
        self.json().ok()
    }

    /// The `x-request-id` (or `request-id`) response header, to quote when reporting an issue.
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
            Failure::Transport(error) => Self::Transport(error),
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

    /// The HTTP status of an API error.
    pub fn status(&self) -> Option<StatusCode> {
        self.api().map(|error| error.status)
    }

    /// The response of an API error.
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
            Self::Api(error) => write!(f, "API error ({}): {}", error.status, error.text()),
            Self::Timeout => f.write_str("request timed out"),
            Self::Transport(error) => write!(f, "transport error: {error}"),
            Self::Decode(error) => write!(f, "unexpected response body: {error}"),
            Self::Request(error) => write!(f, "invalid request: {error}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transport(error) | Self::Decode(error) | Self::Request(error) => Some(&**error),
            Self::Api(_) | Self::Timeout => None,
        }
    }
}

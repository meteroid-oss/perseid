use http_body_util::BodyExt;
use hyper::body::Incoming;
use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug)]
pub struct Error {
    status: Option<http1::StatusCode>,
    message: String,
}
impl Error {
    pub(crate) fn generic(error: impl std::error::Error) -> Self {
        Self {
            status: None,
            message: error.to_string(),
        }
    }
    pub(crate) async fn from_response(status: http1::StatusCode, body: Incoming) -> Self {
        let message = match body.collect().await {
            Ok(body) => String::from_utf8_lossy(&body.to_bytes()).into_owned(),
            Err(error) => error.to_string(),
        };
        Self {
            status: Some(status),
            message,
        }
    }
    pub fn status(&self) -> Option<http1::StatusCode> {
        self.status
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(f)
    }
}
impl std::error::Error for Error {}

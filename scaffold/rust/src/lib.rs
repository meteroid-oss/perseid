//! @@DESCRIPTION@@
//!
//! The entry point is the [`api::@@CLIENT_NAME@@`] client.
#![forbid(unsafe_code)]
pub mod api;
mod configuration;
mod connector;
pub mod error;
pub mod models;
mod request;
#[cfg(feature = "webhooks")]
pub mod webhooks;
pub(crate) use configuration::Configuration;
pub(crate) use connector::make_connector;

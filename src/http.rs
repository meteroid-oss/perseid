//! The agent downloads go through, trusting the system's certificates as curl and cargo do, so
//! proxies with their own CA work.

use std::time::Duration;

use ureq::{
    Agent,
    config::ConfigBuilder,
    tls::{RootCerts, TlsConfig},
    typestate::AgentScope,
};

pub(crate) fn config(timeout: Duration) -> ConfigBuilder<AgentScope> {
    Agent::config_builder()
        .user_agent(concat!("perseid/", env!("CARGO_PKG_VERSION")))
        .timeout_global(Some(timeout))
        .tls_config(
            TlsConfig::builder()
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
}

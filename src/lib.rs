mod api;
pub mod assets;
pub(crate) mod cli_v2;
mod codesamples;
pub mod config;
mod format;
mod fsx;
pub mod generate;
mod generator;
pub mod init;
mod postprocessing;
pub mod pr;
pub mod spec;
mod template;

pub use crate::{
    codesamples::{CodeSample, CodesampleTemplates, generate_codesamples},
    postprocessing::CodegenLanguage,
};

/// The API model templates receive, as pretty JSON.
pub fn inspect(spec: &str, config: &config::Config) -> anyhow::Result<String> {
    Ok(serde_json::to_string_pretty(&spec::api(
        spec,
        &config.filters(),
    )?)?)
}

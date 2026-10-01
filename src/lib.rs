mod api;
pub mod assets;
mod codesamples;
pub mod config;
mod format;
mod fsx;
pub mod generate;
mod generator;
pub mod github;
pub mod init;
mod postprocessing;
pub mod pr;
pub mod scaffold;
pub mod spec;
mod template;
mod value_vec;

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

mod api;
pub mod assets;
pub mod changelog;
pub mod client_name;
mod codesamples;
pub mod config;
mod docs;
mod format;
mod fsx;
pub mod generate;
mod generator;
pub mod github;
mod http;
pub mod init;
mod postprocessing;
pub mod pr;
mod prompt;
mod reserved;
pub mod samples;
pub mod scaffold;
pub mod sizing;
pub mod spec;
mod stainless;
mod template;
mod testcases;
pub mod tools;
mod value_vec;

pub use crate::{
    codesamples::{CodeSample, CodesampleTemplates, generate_codesamples},
    postprocessing::CodegenLanguage,
};

/// The API model templates receive under `filters`, as pretty JSON.
pub fn inspect(spec: &str, filters: &spec::Filters) -> anyhow::Result<String> {
    Ok(serde_json::to_string_pretty(&spec::api(spec, filters)?)?)
}

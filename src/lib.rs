mod api;
pub mod assets;
pub mod changelog;
pub mod client_name;
mod codesamples;
pub mod config;
mod docs;
pub mod docs_data;
mod format;
mod fsx;
pub mod generate;
mod generator;
pub mod github;
mod http;
pub mod init;
pub mod manifest;
pub mod pack;
mod postprocessing;
pub mod pr;
mod prompt;
mod reserved;
pub mod samples;
pub mod scaffold;
pub mod sizing;
pub mod spec;
mod stainless;
pub mod targets;
mod template;
mod testcases;
pub mod tools;
mod value_vec;

pub use crate::{
    codesamples::{CodeSample, CodesampleTemplates, generate_codesamples},
    postprocessing::CodegenLanguage,
};

/// The version of the model templates receive, which a pack names in its `pack.toml`: raised
/// when a change to it can break templates written for the previous one.
pub const MODEL_VERSION: u32 = 1;

/// The API model templates receive under `filters`, as pretty JSON, after its `model_version`.
pub fn inspect(spec: &str, filters: &spec::Filters) -> anyhow::Result<String> {
    let mut model = serde_json::Map::new();
    model.insert("model_version".into(), MODEL_VERSION.into());
    if let serde_json::Value::Object(api) = serde_json::to_value(spec::api(spec, filters)?)? {
        model.extend(api);
    }
    Ok(serde_json::to_string_pretty(&model)?)
}

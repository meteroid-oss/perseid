//! Configured SDK generation through the native generator library.
use crate::{
    api::Api,
    cli_v1::{ExitOnErrorLayer, IncludeMode, get_webhooks},
    generator,
    project::{
        assets::Assets,
        io,
        prepare::{self, Preparation},
    },
};
use anyhow::{Context, Result, ensure};
use camino::Utf8Path;
use clap::Parser;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    sync::atomic::Ordering,
};
use tracing_subscriber::prelude::*;

pub const LANGUAGES: &[&str] = &["rust", "java", "typescript", "python", "go"];
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub template: String,
    pub output_dir: String,
    #[serde(default)]
    pub extra_codegen_args: Vec<String>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LanguageConfig {
    pub task: Vec<Task>,
    pub runtime_output_dir: Option<String>,
    #[serde(default)]
    pub runtime_overrides: BTreeMap<String, String>,
    #[serde(default)]
    pub sdk: BTreeMap<String, Value>,
    #[serde(default)]
    pub extra_codegen_args: Vec<String>,
    #[serde(default)]
    pub format_commands: Option<Vec<String>>,
    #[serde(default)]
    pub check_commands: Vec<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Global {
    pub perseid_version: String,
    pub input_files: Vec<String>,
    pub version_file: Option<String>,
    pub template_overrides: Option<String>,
    pub prepare: Option<Preparation>,
    pub sdk: BTreeMap<String, Value>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Config {
    pub global: Global,
    #[serde(flatten)]
    pub languages: BTreeMap<String, LanguageConfig>,
}

#[derive(Parser, Default)]
struct Filters {
    #[arg(long, value_enum, default_value_t = IncludeMode::OnlyPublic)]
    include_mode: IncludeMode,
    #[arg(short = 'e', long = "exclude-op-id")]
    excluded: Vec<String>,
    #[arg(short = 's', long = "include-op-id")]
    specified: Vec<String>,
}

enum Input {
    OpenApi(Value),
    Ron(String),
}

fn load_api(inputs: &[Input], arguments: &[String]) -> Result<Api> {
    let filters = Filters::try_parse_from(
        std::iter::once("generate".to_owned()).chain(arguments.iter().cloned()),
    )?;
    let excluded = filters.excluded.into_iter().collect();
    let specified = filters.specified.into_iter().collect();
    inputs.iter().try_fold(Api::default(), |api, input| {
        let input = match input {
            Input::Ron(input) => return api.merge(ron::from_str(input)?),
            Input::OpenApi(input) => input,
        };
        let mut spec: aide::openapi::OpenApi = serde_json::from_str(&serde_json::to_string(input)?)
            .context("parsing OpenAPI input")?;
        let webhooks = get_webhooks(&spec);
        let next = Api::new(
            spec.paths.context("No endpoints in spec")?,
            &mut spec.components.take().unwrap_or_default(),
            &webhooks,
            filters.include_mode,
            &excluded,
            &specified,
        )?;
        api.merge(next)
    })
}

pub fn render_runtime(source: &str, sdk: &BTreeMap<String, Value>) -> Result<String> {
    let pattern = regex::Regex::new(r"@@([A-Z_]+)@@")?;
    let mut result = String::new();
    let mut end = 0;
    for capture in pattern.captures_iter(source) {
        let whole = capture.get(0).unwrap();
        result.push_str(&source[end..whole.start()]);
        let key = capture[1].to_lowercase();
        let value = sdk
            .get(&key)
            .with_context(|| format!("Missing SDK setting for {}", &capture[0]))?;
        result.push_str(
            value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string())
                .as_str(),
        );
        end = whole.end();
    }
    result.push_str(&source[end..]);
    Ok(result)
}
fn marked(path: &Path) -> bool {
    fs::read_to_string(path)
        .is_ok_and(|s| s.lines().take(3).any(|line| line.contains("@generated")))
}

pub struct GenerateOptions<'a> {
    pub languages: &'a [String],
    pub inputs: Option<&'a [Value]>,
    pub manifest: &'a str,
    pub coverage_file: &'a str,
}
impl Default for GenerateOptions<'_> {
    fn default() -> Self {
        Self {
            languages: &[],
            inputs: None,
            manifest: "codegen/generated_files.json",
            coverage_file: "codegen/coverage.json",
        }
    }
}

pub fn generate(
    config: &Config,
    root: &Path,
    assets: &Assets,
    options: GenerateOptions<'_>,
) -> Result<()> {
    ensure!(
        config.global.perseid_version == env!("CARGO_PKG_VERSION"),
        "SDK pins Perseid {}; binary is {}",
        config.global.perseid_version,
        env!("CARGO_PKG_VERSION")
    );
    ensure!(
        !config.global.input_files.is_empty(),
        "Configure at least one input spec"
    );
    ensure!(
        config
            .languages
            .keys()
            .all(|k| LANGUAGES.contains(&k.as_str())),
        "Unknown language configuration"
    );
    let selected = if options.languages.is_empty() {
        config.languages.keys().cloned().collect::<Vec<_>>()
    } else {
        options.languages.to_vec()
    };
    ensure!(
        !selected.is_empty() && selected.iter().all(|k| config.languages.contains_key(k)),
        "Select configured languages"
    );
    let mut sdk = config.global.sdk.clone();
    for key in [
        "client_name",
        "package_name",
        "rust_crate",
        "java_package",
        "default_base_url",
        "user_agent_prefix",
        "header_prefix",
        "patch_nullable",
    ] {
        ensure!(sdk.contains_key(key), "Missing SDK setting: {key}");
    }
    if let Some(path) = &config.global.version_file {
        sdk.insert(
            "version".into(),
            fs::read_to_string(io::relative(root, path)?)?.trim().into(),
        );
    }
    let manifest_path = io::relative(root, options.manifest)?;
    let previous = if manifest_path.exists() {
        io::read_json(&manifest_path)?
    } else {
        json!({})
    };
    let mut merged: BTreeMap<String, Vec<String>> = if previous.is_object() {
        serde_json::from_value(previous.clone())?
    } else {
        BTreeMap::new()
    };
    let runtime: BTreeMap<String, BTreeMap<String, String>> =
        serde_json::from_value(io::read_json(&assets.root.join("runtime/manifest.json"))?)?;
    let temporary = tempfile::tempdir()?;
    let stage = temporary.path();
    let templates = stage.join("_templates");
    fs::create_dir_all(&templates)?;
    io::copy_files(
        &assets.root.join("templates"),
        &templates,
        &io::files(&assets.root.join("templates"), false)?,
    )?;
    if let Some(path) = &config.global.template_overrides {
        let path = io::relative(root, path)?;
        io::copy_files(&path, &templates, &io::files(&path, false)?)?;
    }
    let mut inputs = match options.inputs {
        Some(values) => values
            .iter()
            .cloned()
            .map(Input::OpenApi)
            .collect::<Vec<_>>(),
        None => config
            .global
            .input_files
            .iter()
            .map(|p| {
                let path = io::relative(root, p)?;
                match path.extension().and_then(|e| e.to_str()) {
                    Some("ron") => Ok(Input::Ron(fs::read_to_string(path)?)),
                    Some("json") => Ok(Input::OpenApi(io::read_json(&path)?)),
                    _ => anyhow::bail!("Input file extension must be .json or .ron"),
                }
            })
            .collect::<Result<Vec<_>>>()?,
    };
    let mut coverage = None;
    if let Some(preparation) = &config.global.prepare {
        ensure!(inputs.len() == 1, "Spec preparation accepts one input");
        let Input::OpenApi(input) = inputs.remove(0) else {
            anyhow::bail!("Spec preparation requires OpenAPI JSON");
        };
        let (input, report) = prepare::prepare(input, &config.global.input_files[0], preparation)?;
        inputs.push(Input::OpenApi(input));
        coverage = Some(report);
    }
    let (layer, error) = ExitOnErrorLayer::new();
    let subscriber = tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .with(layer);
    let rendered = tracing::subscriber::with_default(
        subscriber,
        || -> Result<BTreeMap<String, Vec<String>>> {
            let mut results = BTreeMap::new();
            for language in &selected {
                let lang = &config.languages[language];
                let mut context = sdk.clone();
                context.extend(lang.sdk.clone());
                let mut paths = Vec::new();
                for task in &lang.task {
                    let output = io::relative(stage, &task.output_dir)?;
                    let template = io::relative(
                        &templates,
                        task.template
                            .strip_prefix("templates/")
                            .unwrap_or(&task.template),
                    )?;
                    let args = lang
                        .extra_codegen_args
                        .iter()
                        .chain(&task.extra_codegen_args)
                        .cloned()
                        .collect::<Vec<_>>();
                    let api = load_api(&inputs, &args)?;
                    let produced = generator::generate_with_output_context(
                        api,
                        template.to_str().context("Non-UTF8 template path")?.into(),
                        Utf8Path::from_path(&output).context("Non-UTF8 output path")?,
                        true,
                        serde_json::to_value(&context)?,
                        Some(&task.output_dir),
                    )?;
                    for path in produced {
                        let path = path
                            .as_std_path()
                            .strip_prefix(stage)
                            .context("Template output escaped stage")?;
                        io::relative(stage, path)?;
                        paths.push(path.to_string_lossy().into_owned());
                    }
                }
                if let Some(directory) = &lang.runtime_output_dir {
                    ensure!(
                        lang.runtime_overrides
                            .keys()
                            .all(|k| runtime[language].contains_key(k)),
                        "Unknown runtime override for {language}"
                    );
                    for (name, output_name) in &runtime[language] {
                        let source = if let Some(path) = lang.runtime_overrides.get(name) {
                            io::relative(root, path)?
                        } else {
                            assets.root.join("runtime").join(language).join(name)
                        };
                        let path =
                            Path::new(directory).join(render_runtime(output_name, &context)?);
                        io::write(
                            &io::relative(stage, &path)?,
                            render_runtime(&fs::read_to_string(source)?, &context)?.as_bytes(),
                        )?;
                        paths.push(path.to_string_lossy().into_owned());
                    }
                }
                ensure!(
                    paths.iter().collect::<BTreeSet<_>>().len() == paths.len(),
                    "Duplicate generated paths for {language}"
                );
                for path in &paths {
                    ensure!(
                        marked(&io::relative(stage, path)?),
                        "Missing generated marker: {path}"
                    );
                }
                paths.sort();
                results.insert(language.clone(), paths);
            }
            Ok(results)
        },
    )?;
    ensure!(
        !error.load(Ordering::SeqCst),
        "Generation logged errors; no SDK output was applied"
    );
    let all = rendered.values().flatten().collect::<Vec<_>>();
    ensure!(
        all.iter().collect::<BTreeSet<_>>().len() == all.len(),
        "Two languages generated the same output path"
    );
    let retained = merged
        .iter()
        .filter(|(k, _)| !selected.contains(k))
        .flat_map(|(_, v)| v.iter())
        .cloned()
        .collect::<BTreeSet<_>>();
    ensure!(
        all.iter().all(|p| !retained.contains(*p)),
        "Output collides with an unselected language"
    );
    let old = if previous.is_array() {
        ensure!(
            selected.len() == config.languages.len(),
            "Legacy manifest migration requires all languages"
        );
        serde_json::from_value::<Vec<Vec<String>>>(previous)?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
    } else {
        selected
            .iter()
            .filter_map(|k| merged.get(k))
            .flatten()
            .cloned()
            .collect()
    };
    // Validate every destination before the first write.
    for path in all
        .iter()
        .map(|p| p.as_str())
        .chain(old.iter().map(String::as_str))
    {
        io::relative(root, path)?;
    }
    for path in &all {
        io::write(
            &io::relative(root, path)?,
            &fs::read(io::relative(stage, path)?)?,
        )?;
    }
    for path in old {
        if !all.contains(&&path) && !retained.contains(&path) {
            let destination = io::relative(root, &path)?;
            if marked(&destination) {
                fs::remove_file(destination)?;
            }
        }
    }
    merged.extend(rendered);
    io::write(&manifest_path, &io::json(&merged)?)?;
    if let Some(coverage) = coverage {
        io::write(
            &io::relative(root, options.coverage_file)?,
            &io::json(&coverage)?,
        )?;
    }
    Ok(())
}

pub fn configured(
    path: &Path,
    languages: &[String],
    no_format: bool,
    checks: bool,
    assets: &Assets,
) -> Result<()> {
    let path = fs::canonicalize(path)?;
    let root = path
        .parent()
        .and_then(Path::parent)
        .context("Config belongs at codegen/codegen.toml")?;
    let config: Config = toml::from_str(&fs::read_to_string(&path)?)?;
    generate(
        &config,
        root,
        assets,
        GenerateOptions {
            languages,
            ..Default::default()
        },
    )?;
    for (name, language) in &config.languages {
        if !languages.is_empty() && !languages.contains(name) {
            continue;
        }
        if !no_format {
            if let Some(commands) = &language.format_commands {
                io::hooks(commands, root, &assets.env())?;
            } else {
                crate::formatting::format(
                    name,
                    root,
                    root,
                    &root.join("codegen/generated_files.json"),
                )?;
            }
        }
        if checks {
            io::hooks(&language.check_commands, root, &assets.env())?;
        }
    }
    Ok(())
}

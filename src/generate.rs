use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::{Context, Result, bail, ensure};
use camino::Utf8PathBuf;
use serde_json::Value;
use tracing_subscriber::prelude::*;

use crate::{
    assets,
    config::{Config, Sdk},
    format, fsx, generator,
    spec::{self, FailOnError},
};

pub struct Options {
    pub check: bool,
    pub format: bool,
}

#[derive(Debug, PartialEq)]
pub enum Change {
    Added(PathBuf),
    Modified(PathBuf),
    Removed(PathBuf),
}

impl std::fmt::Display for Change {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Change::Added(p) => write!(f, "+ {}", p.display()),
            Change::Modified(p) => write!(f, "~ {}", p.display()),
            Change::Removed(p) => write!(f, "- {}", p.display()),
        }
    }
}

/// Where each template renders to, and where the runtime goes, relative to the SDK root.
fn layout(language: &str, context: &Value) -> (PathBuf, Vec<(&'static str, PathBuf)>) {
    let runtime = PathBuf::from(match language {
        "rust" | "typescript" => "src".to_owned(),
        "python" => context["package_name"]
            .as_str()
            .unwrap()
            .replace(['.', '-'], "_"),
        "java" => format!(
            "src/main/java/{}",
            context["java_package"].as_str().unwrap().replace('.', "/")
        ),
        _ => ".".to_owned(),
    });
    let (api, models) = match language {
        "go" => (runtime.clone(), runtime.clone()),
        _ => (runtime.join("api"), runtime.join("models")),
    };
    let summary = match language {
        "rust" | "python" => api.clone(),
        _ => runtime.clone(),
    };
    let mut tasks = vec![
        ("api_resource", api.clone()),
        ("api_summary", summary),
        ("component_type", models.clone()),
    ];
    match language {
        "java" => tasks.push(("operation_options", api)),
        "go" => {}
        _ => tasks.push(("component_type_summary", models)),
    }
    (runtime, tasks)
}

/// Renders templates, then runtime files, into `stage`. Returns their paths relative to it.
fn render(
    config: &Config,
    root: &Path,
    language: &str,
    context: &Value,
    spec: &str,
    stage: &Path,
) -> Result<Vec<PathBuf>> {
    let assets_dir = tempfile::tempdir()?;
    assets::materialize(
        language,
        Some(&config.overrides_dir(root)),
        assets_dir.path(),
    )?;
    let (runtime, tasks) = layout(language, context);
    let extension = match language {
        "rust" => "rs",
        "typescript" => "ts",
        "python" => "py",
        other => other,
    };
    let filters = config.filters();
    let mut produced = Vec::new();
    for (template, output) in tasks {
        let template = assets_dir
            .path()
            .join(format!("templates/{language}/{template}.{extension}.jinja"));
        let template = template.to_str().context("non UTF-8 path")?.to_owned();
        let out = Utf8PathBuf::from_path_buf(stage.join(&output))
            .map_err(|p| anyhow::anyhow!("non UTF-8 path {}", p.display()))?;
        let api = spec::api(spec, &filters)?;
        let paths = generator::generate_with_output_context(
            api,
            template,
            &out,
            true,
            context.clone(),
            Some(output.to_str().unwrap()),
        )?;
        for path in paths {
            produced.push(clean(path.as_std_path().strip_prefix(stage)?));
        }
    }
    let runtime_dir = assets_dir.path().join("runtime").join(language);
    if runtime_dir.is_dir() {
        for file in assets::walk(&runtime_dir)? {
            let name = tokens(file.strip_prefix(&runtime_dir)?.to_str().unwrap(), context)?;
            let content = tokens(&std::fs::read_to_string(&file)?, context)?;
            let path = clean(&runtime.join(name));
            fsx::write(&stage.join(&path), content.as_bytes())?;
            produced.push(path);
        }
    }
    let mut seen = BTreeSet::new();
    for path in &produced {
        ensure!(seen.insert(path), "{} is generated twice", path.display());
        ensure!(
            generated(&stage.join(path)),
            "{} lacks the `@generated` marker in its first lines, templates must emit it",
            path.display()
        );
    }
    Ok(produced)
}

/// Substitutes `@@UPPER_SNAKE@@` tokens with `sdk` context values.
pub(crate) fn tokens(source: &str, context: &Value) -> Result<String> {
    let mut out = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(start) = rest.find("@@") {
        let Some(len) = rest[start + 2..].find("@@") else {
            break;
        };
        let key = &rest[start + 2..start + 2 + len];
        if key.is_empty() || !key.bytes().all(|b| b.is_ascii_uppercase() || b == b'_') {
            out.push_str(&rest[..start + 2]);
            rest = &rest[start + 2..];
            continue;
        }
        let value = context
            .get(key.to_lowercase())
            .with_context(|| format!("unknown runtime token @@{key}@@"))?;
        out.push_str(&rest[..start]);
        match value {
            Value::String(s) => out.push_str(s),
            other => out.push_str(&other.to_string()),
        }
        rest = &rest[start + 4 + len..];
    }
    out.push_str(rest);
    Ok(out)
}

fn clean(path: &Path) -> PathBuf {
    path.components()
        .filter(|c| *c != std::path::Component::CurDir)
        .collect()
}

fn generated(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|s| {
        s.lines()
            .take(3)
            .any(|l| l.to_lowercase().contains("this file is @generated"))
    })
}

/// Generates one SDK into `dir`. With `check`, only reports what would change.
pub fn sdk(
    config: &Config,
    root: &Path,
    sdk: &Sdk,
    dir: &Path,
    spec: &str,
    options: &Options,
) -> Result<Vec<Change>> {
    std::fs::create_dir_all(dir)?;
    let context = config.context(sdk, dir);
    let stage = tempfile::Builder::new()
        .prefix("perseid-stage-")
        .tempdir_in(dir)?;
    let failed = Arc::new(AtomicBool::new(false));
    let subscriber = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_target(false)
                .without_time()
                .with_filter(tracing::level_filters::LevelFilter::ERROR),
        )
        .with(FailOnError(failed.clone()));
    let produced = tracing::subscriber::with_default(subscriber, || {
        render(config, root, sdk.language, &context, spec, stage.path())
    })?;
    if failed.load(Ordering::SeqCst) {
        bail!("{} generation logged errors", sdk.language);
    }
    if options.format {
        format::format(
            sdk.language,
            dir,
            &produced
                .iter()
                .map(|p| stage.path().strip_prefix(dir).unwrap().join(p))
                .collect::<Vec<_>>(),
        )?;
    }
    let mut changes = Vec::new();
    let mut dirs = BTreeSet::new();
    for path in &produced {
        let new = std::fs::read(stage.path().join(path))?;
        match std::fs::read(dir.join(path)) {
            Ok(old) if old == new => {}
            Ok(_) => changes.push((Change::Modified(path.clone()), Some(new))),
            Err(_) => changes.push((Change::Added(path.clone()), Some(new))),
        }
        dirs.insert(path.parent().unwrap_or(Path::new("")).to_owned());
    }
    let produced: BTreeSet<_> = produced.into_iter().collect();
    for relative in dirs {
        let Ok(entries) = std::fs::read_dir(dir.join(&relative)) else {
            continue;
        };
        for entry in entries {
            let path = relative.join(entry?.file_name());
            if dir.join(&path).is_file() && !produced.contains(&path) && generated(&dir.join(&path))
            {
                changes.push((Change::Removed(path), None));
            }
        }
    }
    if !options.check {
        for (change, content) in &changes {
            match (change, content) {
                (Change::Removed(path), _) => std::fs::remove_file(dir.join(path))?,
                (Change::Added(path) | Change::Modified(path), Some(content)) => {
                    fsx::write(&dir.join(path), content)?
                }
                _ => unreachable!(),
            }
        }
    }
    let mut changes: Vec<_> = changes.into_iter().map(|(c, _)| c).collect();
    changes.sort_by_key(|c| match c {
        Change::Added(p) | Change::Modified(p) | Change::Removed(p) => p.clone(),
    });
    Ok(changes)
}

pub fn load_spec(config: &Config, root: &Path) -> Result<String> {
    spec::read(&config.spec, root)
}

pub fn summary(changes: &BTreeMap<String, Vec<Change>>) -> String {
    let listed = changes.values().map(Vec::len).sum::<usize>() <= 30;
    let mut out = String::new();
    for (name, changes) in changes {
        let count = |f: fn(&Change) -> bool| changes.iter().filter(|c| f(c)).count();
        let (added, modified, removed) = (
            count(|c| matches!(c, Change::Added(_))),
            count(|c| matches!(c, Change::Modified(_))),
            count(|c| matches!(c, Change::Removed(_))),
        );
        out += &match changes.len() {
            0 => format!("{name}: up to date\n"),
            _ => format!("{name}: {added} added, {modified} modified, {removed} removed\n"),
        };
        if listed {
            changes.iter().for_each(|c| out += &format!("  {c}\n"));
        }
    }
    out.trim_end().to_owned()
}

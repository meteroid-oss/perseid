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
use itertools::Itertools as _;
use serde_json::Value;
use tracing_subscriber::prelude::*;

use crate::{
    assets,
    config::{Config, Sdk, Source},
    format, fsx, generator,
    spec::{self, FailOnError, Report},
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
        "csharp" => context["package_name"].as_str().unwrap().to_owned(),
        "java" => format!(
            "src/main/java/{}",
            context["java_package"].as_str().unwrap().replace('.', "/")
        ),
        _ => ".".to_owned(),
    });
    let (api, models) = match language {
        "go" => (runtime.clone(), runtime.clone()),
        "csharp" => (runtime.join("Api"), runtime.join("Models")),
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
        "csharp" => {
            tasks.push(("operation_options", api));
            tasks.push(("component_type_summary", models));
        }
        "go" => {}
        _ => tasks.push(("component_type_summary", models)),
    }
    (runtime, tasks)
}

fn extension(language: &str) -> &str {
    match language {
        "rust" => "rs",
        "typescript" => "ts",
        "python" => "py",
        "csharp" => "cs",
        other => other,
    }
}

/// Directories never scanned for stale files: dependencies, build output and VCS metadata.
fn skipped(name: &str) -> bool {
    name.starts_with('.')
        || name.starts_with("perseid-stage-")
        || matches!(
            name,
            "node_modules"
                | "target"
                | "build"
                | "dist"
                | "vendor"
                | "__pycache__"
                | "venv"
                | "bin"
                | "obj"
        )
}

/// Collects `extension` files under `root` (relative to `dir`), so directories that no longer
/// receive any generated file are still checked for leftovers.
fn scan_sources(
    dir: &Path,
    root: &Path,
    extension: &str,
    out: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    let Ok(entries) = std::fs::read_dir(dir.join(root)) else {
        return Ok(());
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let path = clean(&root.join(&name));
        if entry.file_type()?.is_dir() {
            if !skipped(&name.to_string_lossy()) {
                scan_sources(dir, &path, extension, out)?;
            }
        } else if path.extension().is_some_and(|e| e == extension) {
            out.insert(path);
        }
    }
    Ok(())
}

/// Renders templates, then runtime files, into `stage`. Returns their paths relative to it.
fn render(
    config: &Config,
    root: &Path,
    sdk: &Sdk,
    context: &Value,
    spec: &str,
    stage: &Path,
) -> Result<Vec<PathBuf>> {
    let language = sdk.language;
    let assets_dir = tempfile::tempdir()?;
    assets::materialize(
        language,
        Some(&config.overrides_dir(root)),
        assets_dir.path(),
    )?;
    let (runtime, tasks) = layout(language, context);
    let extension = extension(language);
    let filters = config.filters_for(sdk);
    let mut produced = Vec::new();
    let mut api = spec::api(spec, &filters)?;
    let (best_match, untyped) = api.settle_object_unions(context);
    if best_match > 0 {
        tracing::warn!(
            "unions of objects that no property tells apart, decoded as their best-matching \
             variant: {best_match}"
        );
    }
    if untyped > 0 {
        tracing::warn!(
            "unions of objects that no property tells apart, left as untyped JSON: {untyped} \
             (`untagged_unions = \"best-match\"` types them)"
        );
    }
    for (template, output) in tasks {
        let template = assets_dir
            .path()
            .join(format!("templates/{language}/{template}.{extension}.jinja"));
        let template = template.to_str().context("non UTF-8 path")?.to_owned();
        let out = Utf8PathBuf::from_path_buf(stage.join(&output))
            .map_err(|p| anyhow::anyhow!("non UTF-8 path {}", p.display()))?;
        let paths = generator::generate_with_output_context(
            api.clone(),
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
            let relative = file.strip_prefix(&runtime_dir)?;
            let Some(relative) = feature_path(relative, context) else {
                continue;
            };
            let name = tokens(relative.to_str().unwrap(), context)?;
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

/// Runtime files under `features/<name>/` are only installed when `sdk.<name>` is true.
fn feature_path<'a>(relative: &'a Path, context: &Value) -> Option<&'a Path> {
    let mut parts = relative.components();
    if parts.next()?.as_os_str() != "features" {
        return Some(relative);
    }
    let feature = parts.next()?.as_os_str().to_str()?;
    (context.get(feature) == Some(&Value::Bool(true))).then_some(parts.as_path())
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
    std::fs::read_to_string(path).is_ok_and(|s| marked(path, &s))
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
        .with(Report)
        .with(FailOnError(failed.clone()));
    let produced = tracing::subscriber::with_default(subscriber, || {
        render(config, root, sdk, &context, spec, stage.path())
    });
    spec::report_held_back();
    let produced = produced?;
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
    let mut handwritten = Vec::new();
    let mut dirs = BTreeSet::new();
    for path in &produced {
        let new = std::fs::read(stage.path().join(path))?;
        match std::fs::read(dir.join(path)) {
            Ok(old) if old == new => {}
            Ok(_) if !generated(&dir.join(path)) => handwritten.push(path.clone()),
            Ok(_) => changes.push((Change::Modified(path.clone()), Some(new))),
            Err(_) => changes.push((Change::Added(path.clone()), Some(new))),
        }
        dirs.insert(path.parent().unwrap_or(Path::new("")).to_owned());
    }
    ensure!(
        handwritten.is_empty(),
        "refusing to overwrite files without the `@generated` marker: {}. Delete or rename them \
         to let perseid generate these paths",
        handwritten
            .iter()
            .map(|p| p.display().to_string())
            .join(", ")
    );
    let produced: BTreeSet<_> = produced.into_iter().collect();
    let mut candidates = BTreeSet::new();
    for relative in dirs {
        let Ok(entries) = std::fs::read_dir(dir.join(&relative)) else {
            continue;
        };
        for entry in entries {
            candidates.insert(relative.join(entry?.file_name()));
        }
    }
    let (runtime, _) = layout(sdk.language, &context);
    scan_sources(dir, &runtime, extension(sdk.language), &mut candidates)?;
    for path in candidates {
        if dir.join(&path).is_file() && !produced.contains(&path) && generated(&dir.join(&path)) {
            changes.push((Change::Removed(path), None));
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
    ensure!(
        config.push_spec.is_none(),
        "the SDKs are generated in {}, which receives the spec (`push_spec`)",
        config.push_spec.as_deref().unwrap_or_default()
    );
    match config.source() {
        Source::GitHub { repo, path } => {
            let snapshot = crate::config::snapshot(path);
            spec::read(&snapshot, root).with_context(|| {
                format!("{snapshot} holds the spec {repo} pushes: run `perseid setup` to fetch it")
            })
        }
        _ => spec::read(&config.spec, root),
    }
}

/// The paths generating `sdk` writes, relative to its directory.
pub fn planned(config: &Config, root: &Path, sdk: &Sdk, spec: &str) -> Result<Vec<PathBuf>> {
    let stage = tempfile::tempdir()?;
    let context = config.context(sdk, stage.path());
    let subscriber = tracing_subscriber::registry();
    tracing::subscriber::with_default(subscriber, || {
        render(config, root, sdk, &context, spec, stage.path())
    })
}

/// Whether a file's text starts with perseid's marker: `@generated`, or for Go the
/// `// Code generated ... DO NOT EDIT.` line Go tools recognize.
pub fn marked(path: &Path, text: &str) -> bool {
    let go = path.extension().is_some_and(|e| e == "go");
    text.lines().take(3).any(|l| match go {
        true => l.starts_with("// Code generated by perseid"),
        false => l.to_lowercase().contains("this file is @generated"),
    })
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

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

include!(concat!(env!("OUT_DIR"), "/assets.rs"));

/// Embedded files under `prefix`, keyed by their path relative to it.
pub fn under(prefix: &str) -> impl Iterator<Item = (&'static str, &'static [u8])> + '_ {
    FILES.iter().filter_map(move |(path, content)| {
        Some((path.strip_prefix(prefix)?.strip_prefix('/')?, *content))
    })
}

/// Writes the built-in templates and runtime for `language` to `dir`, then layers overrides on top.
pub fn materialize(language: &str, overrides: Option<&Path>, dir: &Path) -> Result<()> {
    for kind in ["templates", "runtime"] {
        let prefix = format!("{kind}/{language}");
        for (path, content) in under(&prefix) {
            crate::fsx::write(&dir.join(kind).join(language).join(path), content)?;
        }
        let Some(custom) = overrides.map(|o| o.join(kind).join(language)) else {
            continue;
        };
        if !custom.is_dir() {
            continue;
        }
        for file in walk(&custom)? {
            let relative = file.strip_prefix(&custom)?;
            let content =
                std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
            crate::fsx::write(&dir.join(kind).join(language).join(relative), &content)?;
        }
    }
    Ok(())
}

/// Copies the built-in templates and runtime of `language` into `dir` for customization.
/// The templates and runtime files of `language`, as `eject` names them.
pub fn ejectable(language: &str) -> Vec<String> {
    let mut files = Vec::new();
    for kind in ["templates", "runtime"] {
        files
            .extend(under(&format!("{kind}/{language}")).map(|(path, _)| format!("{kind}/{path}")));
    }
    files
}

/// Copies the templates and runtime files of `language` named in `only` (all without), skipping
/// those already there.
pub fn eject(language: &str, only: &[String], dir: &Path) -> Result<Vec<PathBuf>> {
    let unknown: Vec<&String> = only
        .iter()
        .filter(|o| !ejectable(language).contains(o))
        .collect();
    anyhow::ensure!(
        unknown.is_empty(),
        "no {} to eject for {language}: `perseid eject {language} --list` lists them",
        unknown
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let mut written = Vec::new();
    for kind in ["templates", "runtime"] {
        for (path, content) in under(&format!("{kind}/{language}")) {
            if !only.is_empty() && !only.contains(&format!("{kind}/{path}")) {
                continue;
            }
            let target = dir.join(kind).join(language).join(path);
            if !target.exists() {
                crate::fsx::write(&target, content)?;
                written.push(target);
            }
        }
    }
    Ok(written)
}

/// The files under `dir`, like [`walk`], leaving out build output, dependencies and git metadata:
/// the folders named `target`, `node_modules` and `.git`.
pub(crate) fn walk_sources(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if !path.is_dir() {
            files.push(path);
        } else if !matches!(
            path.file_name().and_then(|n| n.to_str()),
            Some("target" | "node_modules" | ".git")
        ) {
            files.extend(walk_sources(&path)?);
        }
    }
    files.sort();
    Ok(files)
}

pub fn walk(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            files.extend(walk(&path)?);
        } else {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

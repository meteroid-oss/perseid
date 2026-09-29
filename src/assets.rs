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
pub fn eject(language: &str, dir: &Path) -> Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    for kind in ["templates", "runtime"] {
        for (path, content) in under(&format!("{kind}/{language}")) {
            let target = dir.join(kind).join(language).join(path);
            if !target.exists() {
                crate::fsx::write(&target, content)?;
                written.push(target);
            }
        }
    }
    Ok(written)
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

use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, ensure};

/// Joins a relative path onto `root`, refusing absolute paths and `..` escapes.
pub fn relative(root: &Path, path: impl AsRef<Path>) -> Result<PathBuf> {
    let path = path.as_ref();
    ensure!(
        path.components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir)),
        "{} must be a relative path inside {}",
        path.display(),
        root.display()
    );
    Ok(root.join(path))
}

pub fn write(path: &Path, content: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content).with_context(|| format!("writing {}", path.display()))
}

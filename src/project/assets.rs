use super::io::{files, json, relative, sha, write};
use anyhow::Result;
use std::{collections::BTreeMap, path::PathBuf};
use tempfile::TempDir;
mod embedded {
    include!(concat!(env!("OUT_DIR"), "/assets.rs"));
}

pub struct Assets {
    pub root: PathBuf,
    pub fingerprint: String,
    _temporary: Option<TempDir>,
}
impl Assets {
    pub fn load() -> Result<Self> {
        let (root, temporary) = if let Some(root) = std::env::var_os("PERSEID_DIR") {
            (std::fs::canonicalize(root)?, None)
        } else {
            let temporary = tempfile::tempdir()?;
            for (path, content) in embedded::FILES {
                write(&relative(temporary.path(), path)?, content)?;
            }
            (temporary.path().to_owned(), Some(temporary))
        };
        let mut hashes = BTreeMap::new();
        for directory in ["templates", "runtime", "scripts"] {
            for (path, state) in files(&root.join(directory), true)? {
                hashes.insert(format!("{directory}/{}", path.display()), state.digest);
            }
        }
        let fingerprint = sha(&json(&(embedded::ENGINE, hashes))?);
        Ok(Self {
            root,
            fingerprint,
            _temporary: temporary,
        })
    }
    pub fn env(&self) -> BTreeMap<String, String> {
        BTreeMap::from([(
            "PERSEID_DIR".into(),
            self.root.to_string_lossy().into_owned(),
        )])
    }
}

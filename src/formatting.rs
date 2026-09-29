//! Default formatters operate only on the generated-file ownership manifest.
use crate::project::io;
use anyhow::{Context, Result};
use std::{collections::BTreeMap, path::Path, process::Command};

pub fn format(language: &str, root: &Path, directory: &Path, manifest: &Path) -> Result<()> {
    let owned: BTreeMap<String, Vec<String>> = serde_json::from_value(io::read_json(manifest)?)?;
    let (tool, extension, passes): (&str, &str, Vec<Vec<&str>>) = match language {
        // skip_children keeps rustfmt from following modules into handwritten files.
        "rust" => (
            "rustfmt",
            "rs",
            vec![vec!["--edition", "2021", "--config", "skip_children=true"]],
        ),
        "java" => ("google-java-format", "java", vec![vec!["-i", "-a"]]),
        "typescript" => (
            "biome",
            "ts",
            vec![
                vec![
                    "lint",
                    "--only=organizeImports",
                    "--only=noUnusedImports",
                    "--only=useImportType",
                    "--unsafe",
                    "--write",
                ],
                vec!["format", "--write"],
            ],
        ),
        "python" => (
            "ruff",
            "py",
            vec![
                vec![
                    "check",
                    "--no-respect-gitignore",
                    "--select",
                    "I,F401",
                    "--fix",
                    "--quiet",
                ],
                vec!["format", "--no-respect-gitignore", "--quiet"],
            ],
        ),
        "go" => ("gofmt", "go", vec![vec!["-w"]]),
        _ => unreachable!("validated language"),
    };
    let files = owned
        .get(language)
        .into_iter()
        .flatten()
        .filter(|p| Path::new(p).extension().is_some_and(|ext| ext == extension))
        .map(|p| io::relative(root, p))
        .collect::<Result<Vec<_>>>()?;
    if files.is_empty() {
        return Ok(());
    }
    for args in passes {
        // Avoid platform argv limits on large schemas.
        for batch in files.chunks(32) {
            let output = io::output(Command::new(tool).args(&args).args(batch).current_dir(directory))
                .with_context(|| format!("{language} formatting failed. Install {tool} on PATH (or use the Perseid Docker image), configure format_commands to override, or use --no-format to skip"))?;
            eprint!("{}", String::from_utf8_lossy(&output.stdout));
        }
    }
    Ok(())
}

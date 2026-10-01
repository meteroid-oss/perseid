use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Result, bail};

type Pass = (&'static [&'static [&'static str]], &'static [&'static str]);

/// Pinned versions of the tools perseid runs, which `perseid tools install` downloads.
macro_rules! biome {
    () => {
        "2.1.4"
    };
}
macro_rules! ruff {
    () => {
        "0.14.10"
    };
}
pub const BIOME: &str = biome!();
pub const RUFF: &str = ruff!();
pub const GOOGLE_JAVA_FORMAT: &str = "1.25.2";
pub const CSHARPIER: &str = "1.3.0";
pub const OASDIFF: &str = "1.32.1";

/// Formatter passes per language: candidate commands (first found wins), then arguments.
/// Pinned fallbacks keep output reproducible when the formatter isn't installed globally.
fn passes(language: &str) -> (&'static str, &'static [Pass]) {
    const BIOME: &[&[&str]] = &[
        &["biome"],
        &["npx", "--yes", concat!("@biomejs/biome@", biome!())],
    ];
    const RUFF: &[&[&str]] = &[
        &["ruff"],
        &["uvx", concat!("ruff@", ruff!())],
        &["pipx", "run", concat!("ruff==", ruff!())],
    ];
    match language {
        "rust" => ("rs", &[(&[&["rustfmt"]], &["--edition", "2021"])]),
        "go" => ("go", &[(&[&["gofmt"]], &["-w"])]),
        "java" => ("java", &[(&[&["google-java-format"]], &["-i", "-a"])]),
        "csharp" => (
            "cs",
            &[(
                &[&["csharpier"]],
                &[
                    "format",
                    "--no-cache",
                    "--no-msbuild-check",
                    "--log-level",
                    "Warning",
                ],
            )],
        ),
        "python" => (
            "py",
            &[
                (
                    RUFF,
                    &[
                        "check",
                        "--no-cache",
                        "--no-respect-gitignore",
                        "--fix",
                        "--quiet",
                    ],
                ),
                (
                    RUFF,
                    &[
                        "check",
                        "--no-cache",
                        "--no-respect-gitignore",
                        "--select",
                        "I",
                        "--fix",
                        "--quiet",
                    ],
                ),
                (
                    RUFF,
                    &["format", "--no-cache", "--no-respect-gitignore", "--quiet"],
                ),
            ],
        ),
        "typescript" => (
            "ts",
            &[
                (
                    BIOME,
                    &[
                        "lint",
                        "--only=organizeImports",
                        "--only=noUnusedImports",
                        "--only=useImportType",
                        "--unsafe",
                        "--write",
                    ],
                ),
                (
                    BIOME,
                    &[
                        "format",
                        "--trailing-commas=es5",
                        "--indent-style=space",
                        "--line-width=90",
                        "--write",
                    ],
                ),
            ],
        ),
        _ => ("", &[]),
    }
}

/// Formats `files` (relative to `cwd`), so formatter configs of the SDK are picked up.
pub fn format(language: &str, cwd: &Path, files: &[PathBuf]) -> Result<()> {
    let (extension, passes) = passes(language);
    let files: Vec<_> = files
        .iter()
        .filter(|f| f.extension().is_some_and(|e| e == extension))
        .collect();
    if files.is_empty() {
        return Ok(());
    }
    // Style comes from the flags alone, so an SDK's own biome config can't change or break it.
    let isolated = tempfile::tempdir()?;
    std::fs::write(isolated.path().join("biome.json"), "{}")?;
    let biome_config = format!("--config-path={}", isolated.path().display());
    for (candidates, args) in passes {
        let Some(command) = candidates.iter().find(|c| on_path(c[0])) else {
            eprintln!(
                "warning: `{}` not found, {language} output is not formatted",
                candidates[0][0]
            );
            return Ok(());
        };
        for chunk in files.chunks(200) {
            let output = Command::new(command[0])
                .args(&command[1..])
                .args(*args)
                .args((language == "typescript").then_some(&biome_config))
                .args(chunk)
                .current_dir(cwd)
                .output()?;
            if !output.status.success() {
                bail!(
                    "`{} {}` failed:\n{}{}",
                    command.join(" "),
                    args.join(" "),
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
    }
    Ok(())
}

pub fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| {
            let path = dir.join(program);
            path.is_file() || path.with_extension("exe").is_file()
        })
    })
}

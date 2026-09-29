//! One-time scaffolding; generated SDK files remain owned by the normal runner.
use super::io;
use anyhow::{Result, ensure};
use clap::Args;
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Clone, Debug, Args)]
pub struct Init {
    #[arg(long)]
    pub name: String,
    #[arg(long = "language", required = true, value_parser = ["rust", "java", "typescript", "python", "go"])]
    pub languages: Vec<String>,
    #[arg(long, default_value = "openapi.json")]
    pub spec: String,
    #[arg(long, default_value = ".")]
    pub output: PathBuf,
    /// Put a single SDK at this relative path (e.g. "." in a dedicated SDK repo).
    #[arg(long)]
    pub directory: Option<String>,
    /// Bootstrap a destination repo managed by a separate controller.
    #[arg(long)]
    pub sdk_only: bool,
    /// Public Go module import path; required when selecting Go.
    #[arg(long)]
    pub go_module: Option<String>,
    /// GitHub backend repository (owner/name). --spec is its committed OpenAPI path.
    #[arg(long, conflicts_with = "sdk_only")]
    pub source_repository: Option<String>,
    #[arg(long, default_value = "main")]
    pub source_ref: String,
    /// Also install the notifier in this existing backend checkout.
    #[arg(long, requires_all = ["source_repository", "repository"])]
    pub backend_directory: Option<PathBuf>,
    /// GitHub orchestration repository (owner/name), used by the backend notifier.
    #[arg(long)]
    pub repository: Option<String>,
    /// Default API URL used by generated clients.
    #[arg(long)]
    pub base_url: Option<String>,
    /// Scaffold for local generation without GitHub Actions.
    #[arg(long)]
    pub no_workflows: bool,
}
pub fn run(args: &Init) -> Result<()> {
    ensure!(
        super::config::identifier(&args.name),
        "Use a lowercase project name with letters, digits, underscores or hyphens"
    );
    let languages = args
        .languages
        .iter()
        .collect::<std::collections::BTreeSet<_>>();
    ensure!(
        args.directory.is_none() || languages.len() == 1,
        "--directory requires one language"
    );
    if languages.iter().any(|l| l.as_str() == "go") {
        let module = args.go_module.as_deref().ok_or_else(|| {
            anyhow::anyhow!("Go requires --go-module (e.g. github.com/acme/go-sdk)")
        })?;
        ensure!(
            !module.is_empty()
                && module
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "./_-".contains(c)),
            "Invalid Go module path"
        );
    }
    for repository in [&args.repository, &args.source_repository]
        .into_iter()
        .flatten()
    {
        let parts = repository.split('/').collect::<Vec<_>>();
        ensure!(
            parts.len() == 2
                && parts.iter().all(|p| !p.is_empty()
                    && *p != "."
                    && *p != ".."
                    && p.bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))),
            "Repository must be owner/name"
        );
    }
    ensure!(
        !args.spec.contains("${{") && !args.source_ref.contains("${{"),
        "Source path/ref cannot contain GitHub expressions"
    );
    ensure!(!args.source_ref.is_empty(), "Source ref must not be empty");
    ensure!(
        !(args.no_workflows && args.backend_directory.is_some()),
        "--backend-directory requires workflows"
    );
    let root = std::env::current_dir()?.join(&args.output);
    // Validate all ancestor components too, including an existing output symlink.
    let root = absolute_checked(&root)?;
    io::relative(&root, &args.spec)?;
    let package = args.name.replace('-', "_");
    let client = package
        .split('_')
        .filter(|p| !p.is_empty())
        .map(|p| format!("{}{}", p[..1].to_ascii_uppercase(), &p[1..]))
        .collect::<String>();
    let mut files: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut targets = serde_json::Map::new();
    let mut overrides = serde_json::Map::new();
    for language in &languages {
        let language = *language;
        let directory = args.directory.as_deref().unwrap_or(language);
        let base = io::relative(&root, directory)?;
        let mut add = |path: &str, text: &str| -> Result<()> {
            files.insert(
                io::relative(&base, path)?,
                text.replace("@@PACKAGE@@", &package)
                    .replace("@@CLIENT@@", &client),
            );
            Ok(())
        };
        let check = match language.as_str() {
            "rust" => {
                add("Cargo.toml", include_str!("scaffold/Cargo.toml.txt"))?;
                add("src/lib.rs", include_str!("scaffold/lib.rs.txt"))?;
                add("src/error.rs", include_str!("scaffold/error.rs.txt"))?;
                "cargo check"
            }
            "typescript" => {
                add(
                    "package.json",
                    &serde_json::to_string_pretty(
                        &json!({"name":args.name,"version":"0.1.0","type":"commonjs","main":"dist/index.js","types":"dist/index.d.ts","files":["dist"],"scripts":{"build":"tsc"},"dependencies":{"uuid":"^11.1.1"},"devDependencies":{"typescript":"^5.5.0","@types/node":"^22.0.0"}}),
                    )?,
                )?;
                add(
                    "tsconfig.json",
                    r#"{"compilerOptions":{"strict":true,"target":"es2020","module":"CommonJS","moduleResolution":"node","declaration":true,"outDir":"dist","esModuleInterop":true,"skipLibCheck":true,"lib":["es2020","dom"]},"include":["src/**/*.ts"]}"#,
                )?;
                add("src/util.ts", include_str!("scaffold/util.ts.txt"))?;
                "npm run build"
            }
            "python" => {
                add(
                    "pyproject.toml",
                    "[build-system]\nrequires = [\"hatchling\"]\nbuild-backend = \"hatchling.build\"\n\n[project]\nname = \"@@PACKAGE@@\"\nversion = \"0.1.0\"\nrequires-python = \">=3.9\"\ndependencies = [\"httpx>=0.27,<1\"]\n\n[tool.hatch.build.targets.wheel]\npackages = [\"@@PACKAGE@@\"]\n",
                )?;
                add(
                    &format!("{package}/__init__.py"),
                    "from .api import @@CLIENT@@, @@CLIENT@@Async, @@CLIENT@@Options\n\n__all__ = [\"@@CLIENT@@\", \"@@CLIENT@@Async\", \"@@CLIENT@@Options\"]\n",
                )?;
                add(
                    &format!("{package}/errors.py"),
                    include_str!("scaffold/errors.py.txt"),
                )?;
                add(
                    &format!("{package}/_version.py"),
                    "from importlib.metadata import PackageNotFoundError, version\n\ntry:\n    __version__ = version(\"@@PACKAGE@@\")\nexcept PackageNotFoundError:\n    __version__ = \"0+uninstalled\"\n",
                )?;
                add(&format!("{package}/py.typed"), "")?;
                "python3 -m compileall -q ."
            }
            "go" => {
                add(
                    "go.mod",
                    &format!("module {}\n\ngo 1.22\n", args.go_module.as_ref().unwrap()),
                )?;
                add("errors.go", include_str!("scaffold/errors.go.txt"))?;
                add(
                    "version.go",
                    "package @@PACKAGE@@\n\nconst Version = \"0.1.0\"\n",
                )?;
                "go test ./..."
            }
            "java" => {
                add("build.gradle", include_str!("scaffold/build.gradle.txt"))?;
                add(
                    "settings.gradle",
                    &format!("rootProject.name = '{}'\n", args.name),
                )?;
                add("version.txt", "0.1.0\n")?;
                add(
                    &format!("src/main/java/com/{package}/Version.java"),
                    "package com.@@PACKAGE@@;\n\npublic final class Version {\n    private Version() {}\n    public static final String VERSION = \"0.1.0\";\n}\n",
                )?;
                add(
                    &format!("src/main/java/com/{package}/exceptions/ApiException.java"),
                    include_str!("scaffold/ApiException.java.txt"),
                )?;
                "gradle build"
            }
            _ => unreachable!(),
        };
        targets.insert(
            language.clone(),
            json!({"directory":directory,"prepare":{}}),
        );
        let mut custom = json!({"check_commands":[check],"sdk":{"package_name":package,"client_name":client,"rust_crate":package,"java_package":format!("com.{package}")}});
        if args.sdk_only
            && let Some(base_url) = &args.base_url
        {
            custom["sdk"]["default_base_url"] = base_url.clone().into();
        }
        if language == "typescript" {
            custom["check_commands"] = json!([
                "if test -f package-lock.json; then npm ci --ignore-scripts; else npm install --ignore-scripts; fi",
                check
            ]);
        }
        if language == "go" {
            custom["version_commands"] = json!([format!(
                "printf 'package {package}\\n\\nconst Version = \"%s\"\\n' \"$PERSEID_VERSION\" > version.go"
            )]);
        }
        if language == "java" {
            custom["read_version"] = "cat version.txt".into();
            custom["version_commands"] = json!([
                "printf '%s\\n' \"$PERSEID_VERSION\" > version.txt",
                format!(
                    "printf 'package com.{package};\\n\\npublic final class Version {{\\n    private Version() {{}}\\n    public static final String VERSION = \"%s\";\\n}}\\n' \"$PERSEID_VERSION\" > src/main/java/com/{package}/Version.java"
                )
            ]);
        }
        overrides.insert(language.clone(), custom);
    }
    if !args.sdk_only {
        let source = if let Some(repository) = &args.source_repository {
            json!({"git":format!("https://github.com/{repository}.git"),"ref":args.source_ref,"path":args.spec})
        } else {
            json!({"file":args.spec})
        };
        let mut config = json!({"name":args.name,"perseid_version":env!("CARGO_PKG_VERSION"),"source":source,"targets":targets});
        if let Some(base_url) = &args.base_url {
            config["sdk"] = json!({"default_base_url":base_url});
        }
        if !args.no_workflows {
            for (path, content) in super::bootstrap::files(&languages) {
                files.insert(io::relative(&root, path)?, content);
            }
        }
        files.insert(
            io::relative(&root, "perseid.toml")?,
            toml::to_string_pretty(&config)?,
        );
    }
    files.insert(io::relative(&root, ".perseid/overrides.toml")?, format!("# Destination-owned settings. Template/runtime paths are relative to this repository.\n{}", toml::to_string_pretty(&json!({"targets":overrides}))?));
    let ignore = io::relative(&root, ".gitignore")?;
    if !ignore.exists() {
        let text = io::IGNORED
            .iter()
            .filter(|p| **p != ".git")
            .map(|p| format!("{p}\n"))
            .collect::<String>();
        files.insert(ignore, text + "*.egg-info/\n");
    }
    if let Some(backend) = &args.backend_directory {
        let backend = std::fs::canonicalize(backend)?;
        ensure!(backend.is_dir(), "Backend checkout must be a directory");
        ensure!(
            backend != root,
            "Backend and orchestration repositories must be different"
        );
        files.insert(
            io::relative(&backend, ".github/workflows/notify-openapi.yml")?,
            super::bootstrap::notification(
                args.repository.as_deref().unwrap(),
                &args.source_ref,
                &args.spec,
            ),
        );
    }
    // Validate every destination before writing anything; no force/overwrite mode.
    for path in files.keys() {
        for parent in path.parent().into_iter().flat_map(|p| p.ancestors()) {
            ensure!(
                !parent.exists() || parent.is_dir(),
                "Init needs a directory at {}; no files written",
                parent.display()
            );
        }
        ensure!(
            !path.exists(),
            "Init would overwrite {}; no files written",
            path.display()
        );
    }
    for (path, content) in &files {
        io::write(path, content.as_bytes())?;
    }
    let paths = files
        .keys()
        .map(|p| p.strip_prefix(&root).unwrap_or(p).display().to_string())
        .collect::<Vec<_>>();
    let next = if args.sdk_only {
        Value::String(
            "Configure this destination in the controller, then run sync and generate".into(),
        )
    } else if args.no_workflows {
        json!(["perseid sync", "perseid generate"])
    } else {
        json!([
            "Configure the PERSEID_TOKEN secret for private/cross-repository access",
            "Commit and push the created files to main; Update SDKs opens the first generation PR"
        ])
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({"created":paths,"next":next}))?
    );
    Ok(())
}

/// Normalize parent components before checking each existing ancestor for symlinks.
fn absolute_checked(path: &std::path::Path) -> Result<PathBuf> {
    use std::path::Component;
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => (),
            part => normalized.push(part.as_os_str()),
        }
    }
    let anchor = normalized.ancestors().last().unwrap();
    io::relative(anchor, normalized.strip_prefix(anchor)?)
}

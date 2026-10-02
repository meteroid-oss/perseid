//! `perseid tools`: the native formatters the SDKs need and oasdiff, which sizes their releases,
//! at the versions format.rs pins, downloaded for CI runners and images.

use std::{
    collections::BTreeSet,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail, ensure};

use crate::{
    config::{Sdk, same_repo},
    format,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tool {
    Biome,
    Ruff,
    GoogleJavaFormat,
    Csharpier,
    Oasdiff,
}

impl Tool {
    pub fn name(self) -> &'static str {
        match self {
            Tool::Biome => "biome",
            Tool::Ruff => "ruff",
            Tool::GoogleJavaFormat => "google-java-format",
            Tool::Csharpier => "csharpier",
            Tool::Oasdiff => "oasdiff",
        }
    }

    pub fn version(self) -> &'static str {
        match self {
            Tool::Biome => format::BIOME,
            Tool::Ruff => format::RUFF,
            Tool::GoogleJavaFormat => format::GOOGLE_JAVA_FORMAT,
            Tool::Csharpier => format::CSHARPIER,
            Tool::Oasdiff => format::OASDIFF,
        }
    }
}

/// The formatters of `languages` that don't come with a toolchain, then oasdiff.
pub fn needed<'a>(languages: impl IntoIterator<Item = &'a str>) -> Vec<Tool> {
    let mut tools: BTreeSet<Tool> = languages
        .into_iter()
        .filter_map(|language| match language {
            "typescript" => Some(Tool::Biome),
            "python" => Some(Tool::Ruff),
            "java" => Some(Tool::GoogleJavaFormat),
            "csharp" => Some(Tool::Csharpier),
            _ => None,
        })
        .collect();
    tools.insert(Tool::Oasdiff);
    tools.into_iter().collect()
}

#[derive(Debug, PartialEq, Eq)]
enum Source {
    Binary(String),
    /// A `.tar.gz` holding the binary at `member`.
    Archive {
        url: String,
        member: String,
    },
    /// A jar run by a `java -jar` wrapper, where no native build exists.
    Jar(String),
    DotnetTool,
}

/// Where GitHub release downloads come from, `PERSEID_GITHUB_WEB` in tests.
fn web() -> String {
    std::env::var("PERSEID_GITHUB_WEB")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "https://github.com".into())
        .trim_end_matches('/')
        .to_owned()
}

/// Where `tool` is downloaded from, on `os` and `arch` as `std::env::consts` names them.
fn source(tool: Tool, os: &str, arch: &str) -> Result<Source> {
    let unsupported = || anyhow!("no {} build for {os} {arch}", tool.name());
    let (web, v) = (web(), tool.version());
    let darwin = match os {
        "linux" => false,
        "macos" => true,
        _ => return Err(unsupported()),
    };
    if !matches!(arch, "x86_64" | "aarch64") {
        return Err(unsupported());
    }
    let x64 = arch == "x86_64";
    Ok(match tool {
        Tool::Biome => Source::Binary(format!(
            "{web}/biomejs/biome/releases/download/%40biomejs%2Fbiome%40{v}/biome-{}-{}",
            if darwin { "darwin" } else { "linux" },
            if x64 { "x64" } else { "arm64" },
        )),
        Tool::Ruff => {
            let system = if darwin {
                "apple-darwin"
            } else {
                "unknown-linux-gnu"
            };
            let triple = format!("{arch}-{system}");
            Source::Archive {
                url: format!("{web}/astral-sh/ruff/releases/download/{v}/ruff-{triple}.tar.gz"),
                member: format!("ruff-{triple}/ruff"),
            }
        }
        Tool::GoogleJavaFormat => {
            let base = format!("{web}/google/google-java-format/releases/download/v{v}");
            match (darwin, x64) {
                (false, true) => Source::Binary(format!("{base}/google-java-format_linux-x86-64")),
                (true, false) => Source::Binary(format!("{base}/google-java-format_darwin-arm64")),
                _ => Source::Jar(format!("{base}/google-java-format-{v}-all-deps.jar")),
            }
        }
        Tool::Oasdiff => {
            let platform = match (darwin, x64) {
                (true, _) => "darwin_all",
                (false, true) => "linux_amd64",
                (false, false) => "linux_arm64",
            };
            Source::Archive {
                url: format!(
                    "{web}/oasdiff/oasdiff/releases/download/v{v}/oasdiff_{v}_{platform}.tar.gz"
                ),
                member: "oasdiff".into(),
            }
        }
        Tool::Csharpier => Source::DotnetTool,
    })
}

const ATTEMPTS: u32 = 3;

fn download(url: &str) -> Result<Vec<u8>> {
    let agent: ureq::Agent = crate::http::config(Duration::from_secs(300)).build().into();
    let mut attempt = 1;
    loop {
        let result = agent.get(url).call().and_then(|mut response| {
            response
                .body_mut()
                .with_config()
                .limit(512 * 1024 * 1024)
                .read_to_vec()
        });
        match result {
            Ok(bytes) => return Ok(bytes),
            Err(ureq::Error::StatusCode(code)) if (400..500).contains(&code) => {
                bail!("downloading {url}: HTTP {code}")
            }
            Err(error) if attempt < ATTEMPTS => {
                eprintln!("warning: downloading {url} failed ({error}), retrying");
                std::thread::sleep(Duration::from_secs(2 << attempt));
                attempt += 1;
            }
            Err(error) => return Err(error).with_context(|| format!("downloading {url}")),
        }
    }
}

fn write_executable(path: &Path, content: &[u8]) -> Result<()> {
    let partial = path.with_extension("partial");
    std::fs::write(&partial, content).with_context(|| format!("writing {}", partial.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&partial, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&partial, path)?;
    Ok(())
}

/// Whether `path --version` runs and reports `version`.
fn runs(path: &Path, version: &str) -> bool {
    Command::new(path)
        .arg("--version")
        .output()
        .is_ok_and(|out| {
            out.status.success()
                && [out.stdout, out.stderr]
                    .iter()
                    .any(|o| String::from_utf8_lossy(o).contains(version))
        })
}

fn command(program: &str, args: &[&str]) -> Result<()> {
    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("running `{program}` (is it installed?)"))?;
    ensure!(
        output.status.success(),
        "`{program} {}` failed:\n{}{}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

/// Installs `tool` in `dir` unless it's there at its pinned version, then checks it runs.
pub fn install(tool: Tool, dir: &Path) -> Result<()> {
    let (name, version) = (tool.name(), tool.version());
    let path = dir.join(name);
    if runs(&path, version) {
        println!("✓ {name} {version} (already installed)");
        return Ok(());
    }
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let dir_arg = dir.to_str().context("non UTF-8 install directory")?;
    match source(tool, std::env::consts::OS, std::env::consts::ARCH)? {
        Source::Binary(url) => write_executable(&path, &download(&url)?)?,
        Source::Archive { url, member } => {
            let scratch = tempfile::tempdir()?;
            let archive = scratch.path().join("archive.tar.gz");
            std::fs::write(&archive, download(&url)?)?;
            let into = scratch
                .path()
                .to_str()
                .context("non UTF-8 temporary path")?;
            let file = archive.to_str().context("non UTF-8 temporary path")?;
            command("tar", &["-xzf", file, "-C", into, &member])?;
            write_executable(&path, &std::fs::read(scratch.path().join(&member))?)?;
        }
        Source::Jar(url) => {
            let jar = dir.join(format!("{name}.jar"));
            std::fs::write(&jar, download(&url)?)?;
            let wrapper = format!("#!/bin/sh\nexec java -jar '{}' \"$@\"\n", jar.display());
            write_executable(&path, wrapper.as_bytes())?;
        }
        Source::DotnetTool => {
            if path.exists() {
                command(
                    "dotnet",
                    &["tool", "uninstall", name, "--tool-path", dir_arg],
                )?;
            }
            let args = ["tool", "install", name, "--version", version];
            command("dotnet", &[&args[..], &["--tool-path", dir_arg]].concat())?;
        }
    }
    ensure!(
        runs(&path, version),
        "{} doesn't run, or `{name} --version` doesn't report {version}",
        path.display()
    );
    println!("✓ {name} {version}");
    Ok(())
}

/// Where tools go by default: next to this executable, on `PATH` wherever perseid is.
pub fn default_dir() -> Result<PathBuf> {
    let exe = std::env::current_exe()?.canonicalize()?;
    Ok(exe.parent().context("perseid has no directory")?.to_owned())
}

/// The account and repositories a GitHub App token for the SDKs' pull requests covers: the
/// account of the SDK repositories, and those of its repositories among `hub` (holding
/// perseid.toml, first) and the SDK repositories.
pub fn app_scope(hub: Option<&str>, sdks: &[Sdk]) -> (Option<String>, Vec<String>) {
    let slug = |repo: &str| -> Option<(String, String)> {
        let repo = repo.trim_end_matches('/').trim_end_matches(".git");
        let mut parts = repo.rsplit(['/', ':']);
        let name = parts.next().filter(|n| !n.is_empty())?;
        let owner = parts.next().filter(|o| !o.is_empty())?;
        Some((owner.to_owned(), name.to_owned()))
    };
    let remote: BTreeSet<(String, String)> = sdks
        .iter()
        .filter_map(|s| s.remote())
        .filter(|r| hub.is_none_or(|hub| !same_repo(r, hub)))
        .filter_map(slug)
        .collect();
    let hub = hub.and_then(slug);
    let Some(owner) = remote.first().or(hub.as_ref()).map(|(o, _)| o.clone()) else {
        return (None, vec![]);
    };
    let repositories = hub
        .iter()
        .chain(&remote)
        .filter(|(o, _)| o.eq_ignore_ascii_case(&owner))
        .map(|(_, name)| name.clone())
        .collect();
    (Some(owner), repositories)
}

/// Appends `key=value` lines to the file `$GITHUB_OUTPUT` names.
pub fn github_output(outputs: &[(&str, String)]) -> Result<()> {
    let path = std::env::var_os("GITHUB_OUTPUT")
        .context("`--github-output` needs GITHUB_OUTPUT, set in GitHub Actions")?;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&path)?;
    for (key, value) in outputs {
        ensure!(!value.contains('\n'), "multi-line {key} output");
        writeln!(file, "{key}={value}")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_need_their_formatters_and_oasdiff() {
        assert_eq!(needed(["rust", "go"]), [Tool::Oasdiff]);
        assert_eq!(
            needed(["csharp", "typescript", "java", "python", "typescript"]),
            [
                Tool::Biome,
                Tool::Ruff,
                Tool::GoogleJavaFormat,
                Tool::Csharpier,
                Tool::Oasdiff
            ]
        );
        assert_eq!(Tool::Biome.version(), format::BIOME);
    }

    #[test]
    fn downloads_are_pinned_per_platform() {
        let gh = "https://github.com";
        assert_eq!(
            source(Tool::Biome, "linux", "aarch64").unwrap(),
            Source::Binary(format!(
                "{gh}/biomejs/biome/releases/download/%40biomejs%2Fbiome%40{}/biome-linux-arm64",
                format::BIOME
            ))
        );
        let Source::Archive { url, member } = source(Tool::Ruff, "macos", "x86_64").unwrap() else {
            panic!("ruff ships archives")
        };
        assert!(url.ends_with("/ruff-x86_64-apple-darwin.tar.gz"), "{url}");
        assert_eq!(member, "ruff-x86_64-apple-darwin/ruff");
        assert!(matches!(
            source(Tool::GoogleJavaFormat, "linux", "aarch64").unwrap(),
            Source::Jar(url) if url.ends_with("-all-deps.jar")
        ));
        let Source::Archive { url, .. } = source(Tool::Oasdiff, "linux", "x86_64").unwrap() else {
            panic!("oasdiff ships archives")
        };
        let v = format::OASDIFF;
        assert_eq!(
            url,
            format!("{gh}/oasdiff/oasdiff/releases/download/v{v}/oasdiff_{v}_linux_amd64.tar.gz")
        );
        assert!(source(Tool::Biome, "windows", "x86_64").is_err());
    }

    fn config(text: &str) -> crate::config::Config {
        crate::config::Config::parse(text, "perseid.toml").unwrap()
    }

    #[test]
    fn app_tokens_cover_the_sdk_repositories_of_one_account() {
        let split = config(
            "name = \"Acme\"\nsdks = [\"go\", \"typescript\"]\nrepo = \"acme/api-{lang}\"\n",
        );
        let sdks = split.sdks(&[]).unwrap();
        assert_eq!(
            app_scope(Some("acme/api-sdks"), &sdks),
            (
                Some("acme".into()),
                vec!["api-sdks".into(), "api-go".into(), "api-typescript".into()]
            )
        );
        assert_eq!(
            app_scope(Some("Elsewhere/api"), &sdks),
            (
                Some("acme".into()),
                vec!["api-go".into(), "api-typescript".into()]
            )
        );
        let local = config("name = \"Acme\"\nsdks = [\"go\"]\n");
        assert_eq!(
            app_scope(Some("acme/api"), &local.sdks(&[]).unwrap()),
            (Some("acme".into()), vec!["api".into()])
        );
        assert_eq!(app_scope(None, &local.sdks(&[]).unwrap()), (None, vec![]));
    }
}

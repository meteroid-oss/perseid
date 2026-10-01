//! `perseid app`, `perseid connect` and `perseid status`: the GitHub side of the layout
//! perseid.toml declares, planned then applied with the user's own GitHub credentials, once they
//! agree. Files go through the user's own commits: `perseid init` writes them.

pub(crate) mod api;
mod app;
pub(crate) mod auth;
mod bootstrap;
mod connect;
mod files;
mod layout;
mod link;
mod plan;
mod push;
mod secrets;
mod status;

use std::{
    io::{BufRead, Write},
    path::{Path, PathBuf},
    process::{Command, ExitCode, Stdio},
};

use anyhow::{Context, Result, bail};

pub use connect::{Connect, connect};
pub use files::{Written, write as write_files, write_release_files};
pub use layout::origin_repo;
pub use link::{Auth, Push, PushOn, push_workflow};
pub use plan::TOKEN;
pub use push::{PushSpec, push_spec};
pub use status::status;

use crate::config::Config;
use api::GitHub;

pub struct Options {
    pub yes: bool,
    pub dry_run: bool,
    pub browser: bool,
}

/// Terminal output and questions; `yes` takes every default without asking.
pub struct Ui {
    pub yes: bool,
    browser: bool,
}

impl Ui {
    pub fn new(options: &Options) -> Self {
        Self {
            yes: options.yes,
            browser: options.browser,
        }
    }

    pub fn ok(&self, message: &str) {
        println!("✓ {message}");
    }

    pub fn warn(&self, message: &str) {
        println!("! {message}");
    }

    pub fn fail(&self, message: &str) {
        println!("✗ {message}");
    }

    /// Something the user has to do.
    pub fn say(&self, message: &str) {
        println!("→ {message}");
    }

    pub fn info(&self, message: &str) {
        println!("  {message}");
    }

    pub fn confirm(&self, question: &str, default: bool) -> Result<bool> {
        if self.yes {
            return Ok(default);
        }
        if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
            return crate::prompt::confirm(question, default);
        }
        loop {
            print!("? {question} {} ", if default { "[Y/n]" } else { "[y/N]" });
            std::io::stdout().flush()?;
            let mut line = String::new();
            if std::io::stdin().lock().read_line(&mut line)? == 0 {
                println!();
                return Ok(false);
            }
            match line.trim().to_lowercase().as_str() {
                "" => return Ok(default),
                "y" | "yes" => return Ok(true),
                "n" | "no" => return Ok(false),
                _ => continue,
            }
        }
    }

    pub fn pause(&self, message: &str) -> Result<()> {
        print!("  {message} ");
        std::io::stdout().flush()?;
        std::io::stdin().lock().read_line(&mut String::new())?;
        Ok(())
    }

    pub fn open(&self, url: &str) {
        if !self.browser {
            return;
        }
        let mut command = match std::env::consts::OS {
            "macos" => Command::new("open"),
            "windows" => {
                let mut c = Command::new("cmd");
                c.args(["/C", "start", ""]);
                c
            }
            _ => Command::new("xdg-open"),
        };
        let _ = command
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}

/// `perseid app`: creates or reuses the GitHub App opening the SDK pull requests, installs it on
/// the repositories perseid.toml names and stores its credentials there, once agreed.
pub fn app(config_path: &Path, options: &Options) -> Result<ExitCode> {
    let ui = Ui::new(options);
    let (_, root) = Config::load(config_path)?;
    layout::required_origin_repo(&root, "the repository holding perseid.toml")?;
    let (token, source) = auth::token(&ui)?;
    let api = GitHub::new(Some(token));
    let login = auth::whoami(&api)?;
    ui.ok(&format!("Signed in to GitHub as {login} ({source})"));
    let cx = plan::Session {
        api: &api,
        login: &login,
        app: true,
        collisions: false,
    };
    let mut plan = plan::plan(&cx, config_path)?;
    println!("\n{}\n", plan.diagram);
    plan.print();
    if plan.pending() == 0 {
        println!();
        ui.ok("The App is set up: nothing to change");
        return Ok(ExitCode::SUCCESS);
    }
    if options.dry_run {
        return Ok(ExitCode::from(2));
    }
    if !ui.confirm("Set up the GitHub App?", true)? {
        println!();
        ui.say("Nothing changed on GitHub. To do it yourself:");
        for manual in plan.manual() {
            ui.info(&manual);
        }
        return Ok(ExitCode::SUCCESS);
    }
    println!();
    plan::apply(&api, &mut plan, &ui)?;
    println!();
    ui.ok("sdks.yml and sdk-release.yml now open their pull requests as the App");
    Ok(ExitCode::SUCCESS)
}

fn credentials(api: &GitHub, app: &app::App, hub: &str, repos: &[String], ui: &Ui) -> Result<()> {
    let mut keyless = Vec::new();
    for repo in repos {
        secrets::set_variable(api, repo, "SDK_APP_ID", &app.id)?;
        match &app.pem {
            Some(pem) => secrets::set_secret(api, repo, "SDK_APP_PRIVATE_KEY", pem)?,
            None if !secrets::has_secret(api, repo, "SDK_APP_PRIVATE_KEY")? => {
                keyless.push(repo.as_str())
            }
            None => {}
        }
    }
    if let Some(slug) = &app.slug {
        secrets::set_variable(api, hub, "SDK_APP_SLUG", slug)?;
    }
    let done: Vec<&str> = repos
        .iter()
        .map(String::as_str)
        .filter(|r| !keyless.contains(r))
        .collect();
    if !done.is_empty() {
        ui.ok(&format!(
            "SDK_APP_ID and SDK_APP_PRIVATE_KEY are set on {}",
            done.join(", ")
        ));
    }
    if !keyless.is_empty() {
        ui.warn(&format!(
            "Add a private key of the App (generated on its settings page) as the SDK_APP_PRIVATE_KEY secret of {}",
            keyless.join(", ")
        ));
    }
    Ok(())
}

/// sdks.yml, regenerating the SDKs when the spec changes on `branch`.
pub struct Workflow<'a> {
    pub branch: &'a str,
    pub paths: &'a [String],
    /// perseid.toml's directory.
    pub dir: &'a str,
    /// Also runs daily, at a minute derived from this, for specs fetched from a URL.
    pub daily: Option<&'a str>,
    /// Skips generating until this file exists.
    pub requires: Option<&'a str>,
}

/// `meteroid-oss/perseid`, or its `action` folder, at the tag of this binary's release line:
/// `v0.6` for 0.6.x, since minor releases may break before 1.0, then `v1` for 1.x.
pub fn uses(action: &str) -> String {
    let tag = match env!("CARGO_PKG_VERSION").split('.').collect::<Vec<_>>()[..] {
        ["0", minor, ..] => format!("v0.{minor}"),
        [major, ..] => format!("v{major}"),
        [] => "v0".to_owned(),
    };
    match action {
        "" => format!("meteroid-oss/perseid@{tag}"),
        action => format!("meteroid-oss/perseid/{action}@{tag}"),
    }
}

pub fn workflow(w: &Workflow) -> String {
    let paths = serde_json::to_string(w.paths).unwrap_or_default();
    let schedule = match w.daily {
        Some(seed) => {
            let minute = seed.bytes().map(u32::from).sum::<u32>() % 60;
            format!("  schedule:\n    - cron: '{minute} 6 * * *'\n")
        }
        None => String::new(),
    };
    let action = uses("");
    let step = match w.requires {
        Some(file) => format!("      - if: hashFiles('{file}') != ''\n        uses: {action}\n"),
        None => format!("      - uses: {action}\n"),
    };
    let dir = match w.dir {
        "" => String::new(),
        dir => format!("          working-directory: {dir}\n"),
    };
    format!(
        r#"# Written by `perseid init`: regenerates the SDKs when the spec changes and opens their pull
# requests as the GitHub App set up by `perseid app`, or with the PERSEID_TOKEN secret.
name: SDKs

on:
  push:
    branches: [{branch:?}]
    paths: {paths}
{schedule}  workflow_dispatch:

permissions:
  contents: read

concurrency: sdks

jobs:
  sdks:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v5
{step}        with:
          token: ${{{{ secrets.PERSEID_TOKEN }}}}
          app-id: ${{{{ vars.SDK_APP_ID }}}}
          app-private-key: ${{{{ secrets.SDK_APP_PRIVATE_KEY }}}}
{dir}"#,
        branch = w.branch,
    )
}

fn git(dir: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .stderr(Stdio::null())
        .output()
        .context("running git")?;
    if !output.status.success() {
        bail!("`git {}` failed in {}", args.join(" "), dir.display());
    }
    Ok(output.stdout)
}

fn toplevel(root: &Path) -> Result<PathBuf> {
    let out = git(root, &["rev-parse", "--show-toplevel"])?;
    Ok(PathBuf::from(String::from_utf8(out)?.trim()))
}

/// `root` relative to the repository top, with `/` separators, empty at the top.
fn relative(root: &Path, top: &Path) -> Result<String> {
    let root = root.canonicalize()?;
    let top = top.canonicalize()?;
    let path = root
        .strip_prefix(&top)
        .context("perseid.toml outside its repository")?;
    Ok(path
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/"))
}

fn join(dir: &str, path: &str) -> String {
    let path = path.trim_start_matches("./");
    match dir {
        "" => path.to_owned(),
        dir => format!("{dir}/{path}"),
    }
}

/// What each registry needs before the first release of the SDKs in `config`, held in `hub`.
pub fn publishing(config: &Config, hub: &str) -> Result<Vec<String>> {
    let mut steps = Vec::new();
    for sdk in config.sdks(&[])? {
        let repo = sdk.remote().unwrap_or(hub);
        let context = config.context(&sdk, Path::new("/nonexistent"));
        let text = |key: &str| context[key].as_str().unwrap_or_default().to_owned();
        let publisher = format!("repository {repo}, workflow sdk-release.yml, environment release");
        steps.push(match sdk.language {
            "typescript" => {
                let name = text("npm_package");
                format!(
                    "npm: publish {name} once with an NPM_TOKEN secret on {repo}, then add a trusted publisher ({publisher}) at https://www.npmjs.com/package/{name}/access and delete the token"
                )
            }
            "python" => format!(
                "PyPI: add a pending publisher for {} ({publisher}) at https://pypi.org/manage/account/publishing/",
                text("package_name")
            ),
            "rust" => {
                let name = text("rust_crate");
                format!(
                    "crates.io: publish {name} once with a CARGO_REGISTRY_TOKEN secret on {repo}, then add a trusted publisher ({publisher}) at https://crates.io/crates/{name}/settings and delete the token"
                )
            }
            "java" => format!(
                "Maven Central: verify the namespace of {} at https://central.sonatype.com/publishing/namespaces, then add MAVEN_CENTRAL_USERNAME, MAVEN_CENTRAL_PASSWORD (a user token), GPG_SECRET_KEY and GPG_SECRET_KEY_PASSWORD secrets to {repo}",
                text("java_package")
            ),
            "csharp" => format!(
                "NuGet: add a NUGET_API_KEY secret to {repo}, from https://www.nuget.org/account/apikeys"
            ),
            _ => format!("Go: nothing to set up, {repo} tags publish through the module proxy"),
        });
    }
    Ok(steps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workflows_pin_the_release_line_of_this_binary() {
        let version = env!("CARGO_PKG_VERSION");
        let line = match version.strip_prefix("0.") {
            Some(rest) => format!("v0.{}", rest.split('.').next().unwrap()),
            None => format!("v{}", version.split('.').next().unwrap()),
        };
        assert_eq!(uses(""), format!("meteroid-oss/perseid@{line}"));
        assert_eq!(uses("push"), format!("meteroid-oss/perseid/push@{line}"));
    }

    #[test]
    fn the_workflow_runs_in_the_config_directory() {
        let paths = ["api/openapi.json".to_owned(), "api/perseid.toml".to_owned()];
        let yaml = workflow(&Workflow {
            branch: "main",
            paths: &paths,
            dir: "api",
            daily: None,
            requires: None,
        });
        assert!(
            yaml.contains("paths: [\"api/openapi.json\",\"api/perseid.toml\"]"),
            "{yaml}"
        );
        assert!(!yaml.contains("schedule"), "{yaml}");
        assert!(
            yaml.ends_with(&format!(
                "      - uses: {}
        with:
          token: ${{{{ secrets.PERSEID_TOKEN }}}}
          app-id: ${{{{ vars.SDK_APP_ID }}}}
          app-private-key: ${{{{ secrets.SDK_APP_PRIVATE_KEY }}}}
          working-directory: api
",
                uses("")
            )),
            "{yaml}"
        );
        assert_eq!(join("", "./openapi.json"), "openapi.json");
    }

    #[test]
    fn url_specs_are_fetched_daily() {
        let paths = ["perseid.toml".to_owned()];
        let yaml = workflow(&Workflow {
            branch: "main",
            paths: &paths,
            dir: "",
            daily: Some("acme/api-sdks"),
            requires: Some("openapi.json"),
        });
        assert!(yaml.contains("  schedule:\n    - cron: '"), "{yaml}");
        assert!(
            yaml.contains("permissions:\n  contents: read\n\nconcurrency: sdks\n"),
            "{yaml}"
        );
        assert!(
            yaml.contains(&format!(
                "      - if: hashFiles('openapi.json') != ''\n        uses: {}\n        with:\n",
                uses("")
            )),
            "{yaml}"
        );
        assert!(
            yaml.ends_with("app-private-key: ${{ secrets.SDK_APP_PRIVATE_KEY }}\n"),
            "{yaml}"
        );
        assert!(!yaml.contains("create-github-app-token"), "{yaml}");
    }
}

//! `perseid setup-github` and `perseid status`: the GitHub side of the layout perseid.toml
//! declares, planned then applied from the terminal with the user's own GitHub credentials, once
//! they agree. Files go through the user's own commits: `perseid init` writes them.

mod api;
mod app;
mod auth;
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
pub use files::{Written, write as write_files};
pub use layout::origin_repo;
pub use link::{Auth, Push, PushOn, push_workflow};
pub use push::{PushSpec, push_spec};
pub use status::status;

use crate::config::{Config, Source};
use api::GitHub;

pub const SETUP_BRANCH: &str = "perseid/setup";
const SDKS_WORKFLOW_NAME: &str = "sdks.yml";

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

/// `perseid setup-github`: plans what GitHub lacks for perseid.toml to hold, then applies it
/// once agreed. With `--dry-run`, exits 2 when changes are pending.
pub fn setup_github(config_path: &Path, options: &Options, app: bool) -> Result<ExitCode> {
    let ui = Ui::new(options);
    let (_, root) = Config::load(config_path)?;
    layout::required_origin_repo(&root, "the repository holding perseid.toml")?;
    let (token, source) = auth::token(&ui)?;
    let api = GitHub::new(Some(token));
    let login = auth::whoami(&api, source, true)?;
    ui.ok(&format!("Signed in to GitHub as {login} ({source})"));
    let cx = plan::Session {
        api: &api,
        login: &login,
        app,
        collisions: true,
    };
    let mut plan = plan::plan(&cx, config_path)?;
    println!("\n{}\n", plan.diagram);
    plan.print();
    let pending = plan.pending();
    if pending == 0 {
        println!();
        ui.ok("In sync on GitHub: nothing to change");
        next_steps(&plan, &[], &ui)?;
        return Ok(ExitCode::SUCCESS);
    }
    if options.dry_run {
        return Ok(ExitCode::from(2));
    }
    let question = match pending {
        1 => "Apply this change on GitHub?".to_owned(),
        n => format!("Apply these {n} changes on GitHub?"),
    };
    if !ui.confirm(&question, true)? {
        println!();
        ui.say("Nothing changed on GitHub. To do it yourself:");
        for manual in plan.manual() {
            ui.info(&manual);
        }
        return Ok(ExitCode::SUCCESS);
    }
    println!();
    let pulls = plan::apply(&api, &mut plan, &ui)?;
    println!("\n✓ {}", plan.diagram);
    next_steps(&plan, &pulls, &ui)?;
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

/// The GitHub App token of a workflow: the account and repositories it covers.
pub struct AppToken<'a> {
    /// Another account than the workflow repository's.
    pub owner: Option<&'a str>,
    pub repositories: &'a [String],
}

/// sdks.yml, regenerating the SDKs when the spec changes on `branch`.
pub struct Workflow<'a> {
    pub branch: &'a str,
    pub paths: &'a [String],
    /// The App's token, or the default token when every SDK lives in the workflow's repository.
    pub app: Option<AppToken<'a>>,
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
    let mut with = String::new();
    if w.app.is_some() {
        with += "          token: ${{ steps.app.outputs.token }}\n";
    }
    if !w.dir.is_empty() {
        with += &format!("          working-directory: {}\n", w.dir);
    }
    let action = uses("");
    let condition = match w.requires {
        Some(file) => format!("      - if: hashFiles('{file}') != ''\n        uses: {action}\n"),
        None => format!("      - uses: {action}\n"),
    };
    let perseid = match with.is_empty() {
        true => condition,
        false => format!("{condition}        with:\n{with}"),
    };
    let (header, permissions, app) = match &w.app {
        Some(app) => (
            "opens\n# their pull requests with a token of the GitHub App set as SDK_APP_ID and SDK_APP_PRIVATE_KEY.",
            "contents: read",
            format!(
                r#"      # App installation tokens expire after an hour, so they are minted on each run.
      - id: app
        uses: actions/create-github-app-token@v2
        with:
          app-id: ${{{{ vars.SDK_APP_ID }}}}
          private-key: ${{{{ secrets.SDK_APP_PRIVATE_KEY }}}}
          owner: {}
          repositories: {}
"#,
                app.owner.unwrap_or("${{ github.repository_owner }}"),
                app.repositories.join(","),
            ),
        ),
        None => (
            "opens\n# their pull requests in this repository with the default token.",
            "contents: write\n  pull-requests: write",
            String::new(),
        ),
    };
    format!(
        r#"# Written by `perseid init`: regenerates the SDKs when the spec changes and {header}
name: SDKs

on:
  push:
    branches: [{branch:?}]
    paths: {paths}
{schedule}  workflow_dispatch:

permissions:
  {permissions}

concurrency:
  group: sdks
  cancel-in-progress: false

jobs:
  sdks:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v5
{app}{perseid}"#,
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

fn next_steps(plan: &plan::Plan, pulls: &[String], ui: &Ui) -> Result<()> {
    let (config, hub) = (plan.config(), plan.hub.as_str());
    println!("\nNext steps");
    let mut step = 0;
    let mut item = |text: String| {
        step += 1;
        ui.info(&format!("{step}. {text}"));
    };
    if plan.unpushed {
        item(format!(
            "Commit and push the files `perseid init` wrote here: {SDKS_WORKFLOW_NAME} runs once it is on the default branch of {hub}"
        ));
    }
    for pull in pulls {
        item(format!("Merge {pull}: it adds the release files there"));
    }
    for sdk in config.sdks(&[])? {
        let repo = sdk.remote().unwrap_or(hub);
        let context = config.context(&sdk, Path::new("/nonexistent"));
        let text = |key: &str| context[key].as_str().unwrap_or_default().to_owned();
        let publisher = format!("repository {repo}, workflow sdk-release.yml, environment release");
        item(match sdk.language {
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
    item(match (config.source(), plan.awaits_spec) {
        (Source::Url(_), _) => {
            "Nothing else: perseid checks the spec every day, or when you run sdks.yml".to_owned()
        }
        (_, true) => format!(
            "In the repository that holds your OpenAPI spec, run `npx perseid connect {hub}`: it pushes the spec here, which opens the SDK pull requests"
        ),
        (Source::File(file), false) => {
            format!("Change {file}: perseid opens the SDK pull requests")
        }
    });
    Ok(())
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
        let repositories = ["api".to_owned(), "acme-node".to_owned()];
        let yaml = workflow(&Workflow {
            branch: "main",
            paths: &paths,
            app: Some(AppToken {
                owner: None,
                repositories: &repositories,
            }),
            dir: "api",
            daily: None,
            requires: None,
        });
        assert!(
            yaml.contains("paths: [\"api/openapi.json\",\"api/perseid.toml\"]"),
            "{yaml}"
        );
        assert!(yaml.contains("repositories: api,acme-node\n"), "{yaml}");
        assert!(
            yaml.contains("owner: ${{ github.repository_owner }}\n"),
            "{yaml}"
        );
        assert!(
            yaml.ends_with("          working-directory: api\n"),
            "{yaml}"
        );
        assert_eq!(join("", "./openapi.json"), "openapi.json");
    }

    #[test]
    fn url_specs_are_fetched_daily_with_the_default_token() {
        let paths = ["perseid.toml".to_owned()];
        let yaml = workflow(&Workflow {
            branch: "main",
            paths: &paths,
            app: None,
            dir: "",
            daily: Some("acme/api-sdks"),
            requires: Some("openapi.json"),
        });
        assert!(yaml.contains("  schedule:\n    - cron: '"), "{yaml}");
        assert!(
            yaml.contains("contents: write\n  pull-requests: write\n"),
            "{yaml}"
        );
        assert!(
            yaml.ends_with(&format!(
                "      - if: hashFiles('openapi.json') != ''\n        uses: {}\n",
                uses("")
            )),
            "{yaml}"
        );
    }
}

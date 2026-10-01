//! `perseid setup` and `perseid status`: the GitHub side of the layout perseid.toml declares,
//! planned then applied from the terminal with the user's own GitHub credentials.

mod api;
mod app;
mod auth;
mod bootstrap;
mod connect;
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
pub use layout::origin_repo;
pub use link::{Auth, Push, PushOn, push_workflow};
pub use push::{PushSpec, push_spec};
pub use status::status;

use crate::config::{Config, Sdk, Source};
use api::GitHub;

pub const SETUP_BRANCH: &str = "perseid/setup";

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

/// `perseid setup`: plans what GitHub lacks for perseid.toml to hold, then applies it once
/// confirmed. With `--dry-run`, exits 2 when changes are pending.
pub fn setup(config_path: &Path, options: &Options) -> Result<ExitCode> {
    let ui = Ui::new(options);
    let (token, source) = auth::token(&ui)?;
    let api = GitHub::new(Some(token));
    let login = auth::whoami(&api, source)?;
    ui.ok(&format!("Signed in to GitHub as {login} ({source})"));
    let cx = plan::Session {
        api: &api,
        login: &login,
        collisions: true,
    };
    let plan = plan::plan(&cx, config_path)?;
    println!("\n{}\n", plan.diagram);
    plan.print();
    let pending = plan.pending();
    if pending == 0 {
        println!();
        ui.ok("In sync: nothing to change");
        if plan.awaits_spec {
            connect_hint(&plan);
        }
        return Ok(ExitCode::SUCCESS);
    }
    if options.dry_run {
        return Ok(ExitCode::from(2));
    }
    let question = match pending {
        1 => "Apply this change?".to_owned(),
        n => format!("Apply these {n} changes?"),
    };
    if !ui.confirm(&question, true)? {
        bail!("nothing changed");
    }
    println!();
    let mut plan = plan;
    let pulls = plan::apply(&api, &mut plan, &ui)?;
    println!("\n✓ {}", plan.diagram);
    checklist(&plan, &pulls, &ui)?;
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
    let condition = match w.requires {
        Some(file) => format!(
            "      - if: hashFiles('{file}') != ''\n        uses: meteroid-oss/perseid@v0\n"
        ),
        None => "      - uses: meteroid-oss/perseid@v0\n".to_owned(),
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
        r#"# Written by `perseid setup`: regenerates the SDKs when the spec changes and {header}
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

/// The new files of this checkout a setup commits: spec, release files, SDKs generated here.
fn local_files(top: &Path, dir: &str, config: &Config, sdks: &[Sdk]) -> Result<Vec<String>> {
    let mut specs = Vec::new();
    if config.release != Some(false) && sdks.iter().any(|s| s.repo.is_none()) {
        specs.extend(
            [
                "release-please-config.json",
                ".release-please-manifest.json",
                crate::scaffold::RELEASE_WORKFLOW,
            ]
            .map(str::to_owned),
        );
    }
    if let Source::File(file) = config.source() {
        specs.push(join(dir, file));
    }
    specs.extend(
        sdks.iter()
            .filter(|s| s.repo.is_none())
            .map(|s| join(dir, &s.path)),
    );
    let mut args = vec![
        "status",
        "--porcelain=v1",
        "-z",
        "--untracked-files=all",
        "--",
    ];
    args.extend(specs.iter().map(String::as_str));
    let status = git(top, &args)?;
    let mut files = Vec::new();
    for entry in status.split(|b| *b == 0).filter(|e| e.len() > 3) {
        if entry.starts_with(b"??") || entry.starts_with(b"A") {
            files.push(String::from_utf8_lossy(&entry[3..]).into_owned());
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

fn executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.file_name().is_some_and(|n| n == "gradlew")
    }
}

/// Stages the committed local files, so pulling the merged commit doesn't trip over them.
fn stage(top: &Path, paths: &[String]) -> Result<()> {
    let mut args = vec!["add", "--"];
    args.extend(paths.iter().map(String::as_str));
    git(top, &args)?;
    Ok(())
}

fn connect_hint(plan: &plan::Plan) {
    println!(
        "\nNext: in the repository that holds your OpenAPI spec, run `npx perseid connect {}`",
        plan.hub
    );
}

fn checklist(plan: &plan::Plan, pulls: &[String], ui: &Ui) -> Result<()> {
    let (config, hub) = (plan.config(), plan.hub.as_str());
    println!("\nNext steps");
    let mut step = 0;
    let mut item = |text: String| {
        step += 1;
        ui.info(&format!("{step}. {text}"));
    };
    match pulls {
        [] => {}
        [one] => item(format!(
            "Merge {one}, then `git pull`: perseid staged the files it adds here"
        )),
        [first, rest @ ..] => item(format!(
            "Merge {first} first, then {}, and `git pull`",
            rest.join(", ")
        )),
    }
    for sdk in config.sdks(&[])? {
        let repo = sdk.repo.as_deref().unwrap_or(hub);
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
            yaml.ends_with("      - if: hashFiles('openapi.json') != ''\n        uses: meteroid-oss/perseid@v0\n"),
            "{yaml}"
        );
    }
}

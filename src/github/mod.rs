//! `perseid init --github`: SDK repositories, a GitHub App and the workflow opening SDK pull
//! requests, set up from the terminal with the user's own GitHub credentials.

mod api;
mod app;
mod auth;
mod bootstrap;
mod layout;
mod secrets;

use std::{
    collections::BTreeSet,
    io::{BufRead, IsTerminal, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Context, Result, bail};
use heck::ToKebabCase;

pub use layout::{plan as layout, spec_repo};

use crate::config::{Config, Sdk};
use api::GitHub;
use bootstrap::{File, Outcome};

pub const SETUP_BRANCH: &str = "perseid/setup";

pub struct Options {
    /// Account of the SDK repositories and the App, the spec repository's by default.
    pub owner: Option<String>,
    pub yes: bool,
    pub browser: bool,
    pub push: bool,
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
        loop {
            print!("? {question} {} ", if default { "[Y/n]" } else { "[y/N]" });
            std::io::stdout().flush()?;
            let mut line = String::new();
            if std::io::stdin().lock().read_line(&mut line)? == 0 {
                println!();
                return Ok(default);
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

/// Whether plain `perseid init` should offer the GitHub setup.
pub fn offered(root: &Path) -> bool {
    std::io::stdin().is_terminal() && spec_repo(root).is_some()
}

pub fn setup(config_path: &Path, options: &Options) -> Result<()> {
    let ui = Ui::new(options);
    let (config, root) = Config::load(config_path)?;
    let spec = layout::required_spec_repo(&root)?;
    println!("\nSetting up GitHub automation for {spec}");
    let (token, source) = auth::token(&ui)?;
    let api = GitHub::new(Some(token));
    let login = auth::whoami(&api, source)?;
    ui.ok(&format!("Signed in to GitHub as {login} ({source})"));

    let spec_info = api.get(&format!("/repos/{spec}"))?;
    let base = bootstrap::default_branch(&spec_info);
    let spec_owner = spec.split('/').next().unwrap_or_default().to_owned();
    let sdks = config.sdks(&[])?;
    let remote: BTreeSet<&str> = sdks.iter().filter_map(|s| s.repo).collect();
    let owner = options
        .owner
        .clone()
        .or_else(|| {
            remote
                .first()
                .and_then(|r| r.split_once('/'))
                .map(|(o, _)| o.to_owned())
        })
        .unwrap_or_else(|| spec_owner.clone());
    if let Some(other) = remote.iter().find(|r| !r.starts_with(&format!("{owner}/"))) {
        bail!(
            "{other} isn't owned by {owner}: the App's token covers the repositories of one account"
        );
    }
    if owner != spec_owner && sdks.iter().any(|s| s.repo.is_none()) {
        bail!(
            "SDKs kept in {spec} need the App on {spec_owner}, not {owner}: set `repo` for each of them"
        );
    }
    let owner = app::owner(&api, &owner)?;
    if !owner.organization && !owner.login.eq_ignore_ascii_case(&login) {
        bail!(
            "{} is another user's account: GitHub only lets {login} create repositories and Apps for itself or its organizations",
            owner.login
        );
    }

    create_missing(&api, &remote, &owner, &spec_info, &config, &ui)?;
    for repo in &remote {
        let held: Vec<&Sdk> = sdks.iter().filter(|s| s.repo == Some(repo)).collect();
        bootstrap::release(&api, &config, repo, &held, &ui)?;
    }

    let name = config.name.to_kebab_case();
    let app = app::obtain(&api, &spec, &owner, &name, &ui)?;
    let mut configured: Vec<String> = vec![spec.clone()];
    configured.extend(remote.iter().map(|r| r.to_string()));
    credentials(&api, &app, &spec, &configured, &ui)?;
    let installed: Vec<String> = configured
        .iter()
        .filter(|r| {
            let prefix = format!("{}/", owner.login.to_lowercase());
            r.to_lowercase().starts_with(&prefix)
        })
        .cloned()
        .collect();
    app::install(&app, &owner, &installed, &ui)?;

    let top = toplevel(&root)?;
    let dir = relative(&root, &top)?;
    let spec_path = (!config.spec.contains("://")).then(|| join(&dir, &config.spec));
    let triggers: Vec<String> = spec_path
        .into_iter()
        .chain([join(&dir, crate::config::FILE)])
        .collect();
    let names: Vec<String> = installed
        .iter()
        .map(|r| r.rsplit('/').next().unwrap_or(r).to_owned())
        .collect();
    let same_owner = owner.login.eq_ignore_ascii_case(&spec_owner);
    let yaml = workflow(
        &base,
        &triggers,
        (!same_owner).then_some(owner.login.as_str()),
        &names,
        &dir,
    );
    let mut files = vec![File {
        path: ".github/workflows/sdks.yml".into(),
        content: yaml.into_bytes(),
        executable: false,
    }];
    let local = local_files(&top, &dir, &config, &sdks)?;
    for path in &local {
        let absolute = top.join(path);
        files.push(File {
            path: path.clone(),
            content: std::fs::read(&absolute).with_context(|| format!("reading {path}"))?,
            executable: executable(&absolute),
        });
    }
    let pr = deliver(&api, &spec, &base, &files, options.push, &app, &ui)?;
    if pr.is_some() || options.push {
        stage(&top, &local)?;
    }
    checklist(&config, &sdks, &spec, &base, pr.as_deref(), &ui);
    Ok(())
}

fn create_missing(
    api: &GitHub,
    remote: &BTreeSet<&str>,
    owner: &app::Owner,
    spec_info: &serde_json::Value,
    config: &Config,
    ui: &Ui,
) -> Result<()> {
    let mut missing = Vec::new();
    for repo in remote {
        if api.find(&format!("/repos/{repo}"))?.is_none() {
            missing.push(*repo);
        }
    }
    if missing.is_empty() {
        return Ok(());
    }
    let visibility = spec_info["visibility"]
        .as_str()
        .unwrap_or(if spec_info["private"] == true {
            "private"
        } else {
            "public"
        });
    let question = format!("Create {} ({visibility})?", missing.join(", "));
    if !ui.confirm(&question, true)? {
        bail!("the SDK repositories must exist: create them, or change `repo` in perseid.toml");
    }
    let path = match owner.organization {
        true => format!("/orgs/{}/repos", owner.login),
        false => "/user/repos".to_owned(),
    };
    for repo in missing {
        let name = repo.split_once('/').map_or(repo, |(_, n)| n);
        let mut body = serde_json::json!({
            "name": name,
            "description": format!("{} API SDK, generated by perseid", config.name),
            "private": visibility != "public",
            "has_wiki": false,
        });
        if owner.organization {
            body["visibility"] = visibility.into();
        }
        api.post(&path, body)?;
        ui.ok(&format!("Created {repo}"));
    }
    Ok(())
}

fn credentials(api: &GitHub, app: &app::App, spec: &str, repos: &[String], ui: &Ui) -> Result<()> {
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
        secrets::set_variable(api, spec, "SDK_APP_SLUG", slug)?;
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

/// The workflow regenerating the SDKs when the spec or perseid.toml change on `branch`.
fn workflow(
    branch: &str,
    paths: &[String],
    owner: Option<&str>,
    repositories: &[String],
    dir: &str,
) -> String {
    let list = |items: &[String]| serde_json::to_string(items).unwrap_or_default();
    let owner = owner.map_or("${{ github.repository_owner }}".to_owned(), str::to_owned);
    let directory = match dir {
        "" => String::new(),
        dir => format!("          working-directory: {dir}\n"),
    };
    format!(
        r#"# Written by `perseid init --github`: regenerates the SDKs when the spec changes and opens
# their pull requests with a token of the GitHub App set as SDK_APP_ID and SDK_APP_PRIVATE_KEY.
name: SDKs

on:
  push:
    branches: [{branch:?}]
    paths: {paths}
  workflow_dispatch:

permissions:
  contents: read

concurrency:
  group: sdks
  cancel-in-progress: false

jobs:
  sdks:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v5
      # App installation tokens expire after an hour, so they are minted on each run.
      - id: app
        uses: actions/create-github-app-token@v2
        with:
          app-id: ${{{{ vars.SDK_APP_ID }}}}
          private-key: ${{{{ secrets.SDK_APP_PRIVATE_KEY }}}}
          owner: {owner}
          repositories: {repositories}
      - uses: meteroid-oss/perseid@v0
        with:
          token: ${{{{ steps.app.outputs.token }}}}
{directory}"#,
        paths = list(paths),
        repositories = repositories.join(","),
    )
}

/// Commits the setup files to the spec repository, returning the pull request opened for them.
fn deliver(
    api: &GitHub,
    spec: &str,
    base: &str,
    files: &[File],
    push: bool,
    app: &app::App,
    ui: &Ui,
) -> Result<Option<String>> {
    let message = "ci: open SDK pull requests on spec changes";
    let branch = if push { base } else { SETUP_BRANCH };
    let extra = match files.len() {
        1 => String::new(),
        n => format!(" and {} more file{}", n - 1, if n == 2 { "" } else { "s" }),
    };
    match bootstrap::commit(api, spec, base, branch, files, message)? {
        Outcome::Unchanged => {
            ui.ok(&format!("{spec} already has sdks.yml on {base}"));
            Ok(None)
        }
        Outcome::Committed if push => {
            ui.ok(&format!("Pushed sdks.yml{extra} to {spec} {base}"));
            Ok(None)
        }
        Outcome::Committed => {
            let bot = app.slug.as_deref().unwrap_or("the GitHub App");
            let paths: Vec<String> = files.iter().map(|f| format!("- `{}`", f.path)).collect();
            let body = format!(
                "Set up by `perseid init --github`: when the spec changes on `{base}`, `.github/workflows/sdks.yml` regenerates the SDKs and opens their pull requests as {bot}.\n\n{}",
                paths.join("\n")
            );
            let (url, opened) = bootstrap::pull_request(api, spec, base, branch, message, &body)?;
            match opened {
                true => ui.ok(&format!("Opened {url} with sdks.yml{extra}")),
                false => ui.ok(&format!("{url} holds sdks.yml{extra}")),
            }
            Ok(Some(url))
        }
    }
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

/// perseid.toml, and the new files `perseid init` writes: spec, SDK skeletons, release files.
fn local_files(top: &Path, dir: &str, config: &Config, sdks: &[Sdk]) -> Result<Vec<String>> {
    let mut specs: Vec<String> = [
        "release-please-config.json",
        ".release-please-manifest.json",
        ".github/workflows/sdk-release.yml",
    ]
    .iter()
    .map(|p| join(dir, p))
    .collect();
    if !config.spec.contains("://") {
        specs.push(join(dir, &config.spec));
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
    let mut files = vec![join(dir, crate::config::FILE)];
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

fn checklist(config: &Config, sdks: &[Sdk], spec: &str, base: &str, pr: Option<&str>, ui: &Ui) {
    println!("\nNext steps");
    let mut step = 0;
    let mut item = |text: String| {
        step += 1;
        ui.info(&format!("{step}. {text}"));
    };
    if let Some(url) = pr {
        item(format!(
            "Merge {url}, then `git pull`: perseid staged the files it adds here"
        ));
    }
    for sdk in sdks {
        let repo = sdk.repo.unwrap_or(spec);
        let context = config.context(sdk, Path::new("/nonexistent"));
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
    item(format!(
        "Push a spec change to {base}: perseid opens the SDK pull requests"
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_workflow_runs_in_the_config_directory() {
        let yaml = workflow(
            "main",
            &["api/openapi.json".into(), "api/perseid.toml".into()],
            None,
            &["api".into(), "acme-node".into()],
            "api",
        );
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
}

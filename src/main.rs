use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::ExitCode,
};

use anyhow::{Context, Result, bail};
use clap::{ArgAction, Parser, Subcommand};
use perseid::{
    config::{self, Config, LANGUAGES},
    generate::{self, Options},
    github::{Auth, PushOn},
    init,
    pr::{self, Bump},
    scaffold, sizing, targets, tools,
};

/// OpenAPI in, idiomatic SDKs out: Rust, TypeScript, Python, Go, Java and C#.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Path to the project file.
    #[arg(short, long, global = true, default_value = config::FILE)]
    config: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Args)]
struct Apply {
    /// Apply the plan without asking.
    #[arg(long, short)]
    yes: bool,
    /// Print the plan and exit: 0 when in sync, 2 when changes are pending.
    #[arg(long, conflicts_with = "yes")]
    dry_run: bool,
    /// Print URLs instead of opening them in a browser.
    #[arg(long)]
    no_browser: bool,
}

impl Apply {
    fn options(&self) -> perseid::github::Options {
        perseid::github::Options {
            yes: self.yes,
            dry_run: self.dry_run,
            browser: !self.no_browser,
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// Write perseid.toml (which SDKs, where they live) and the workflows regenerating and releasing
    /// them, in the repository that will hold the SDKs. Run again to refresh the workflows.
    Init {
        /// SDKs to generate, comma-separated (asked when omitted).
        #[arg(long, value_delimiter = ',', value_parser = LANGUAGES)]
        sdks: Vec<String>,
        /// Repository of each SDK, `owner/name-{lang}`, or `owner/name` holding them all (default:
        /// a folder each, here).
        #[arg(long)]
        repo: Option<String>,
        /// OpenAPI document, path or URL (default: detected, or `openapi.json` pushed by `perseid connect`).
        #[arg(long)]
        spec: Option<String>,
        /// Client name (default: derived from the spec title).
        #[arg(long)]
        name: Option<String>,
        /// API base URL (default: first server of the spec).
        #[arg(long)]
        base_url: Option<String>,
        /// SPDX license of the SDKs, such as MIT or Apache-2.0 (default: the spec's, else asked).
        #[arg(long)]
        license: Option<String>,
        /// Stainless config to migrate from (stainless.yml), path or URL: its targets, packages,
        /// repositories, environments, client settings, pagination and method names, without
        /// asking. What doesn't map is listed.
        #[arg(long, value_name = "STAINLESS_YML")]
        from: Option<String>,
    },
    /// Generate the SDKs perseid.toml lists, from its spec, starting each from its package skeleton.
    Generate {
        /// Languages to generate (default: all configured).
        #[arg(value_parser = LANGUAGES)]
        languages: Vec<String>,
        /// OpenAPI document to generate from, path or URL, over `spec` of perseid.toml.
        #[arg(long)]
        spec: Option<String>,
        /// Fail if generated files are out of date, without writing anything.
        #[arg(long, conflicts_with = "pr")]
        check: bool,
        /// Write every SDK under this directory, in a folder each, without checking out its
        /// repository.
        #[arg(long, conflicts_with = "pr")]
        out: Option<PathBuf>,
        /// Commit the generated files on top of the upstream branch to `perseid/update`, in a
        /// temporary worktree, and open a pull request, with `GH_TOKEN` (else `GITHUB_TOKEN`,
        /// gh's token or a browser login). An update of several SDKs too large for release-please
        /// gets one per SDK, from `perseid/update-<language>`.
        #[arg(long)]
        pr: bool,
        /// Release size the pull request asks for, as its conventional-commit type.
        #[arg(
            long,
            value_enum,
            requires = "pr",
            default_value = "auto",
            env = "PERSEID_BUMP"
        )]
        bump: Bump,
        /// Previous OpenAPI document `--bump auto` compares with (default: the spec file at
        /// `GITHUB_EVENT_BEFORE`, else at the previous commit).
        #[arg(long, requires = "pr", env = "PERSEID_BASE_SPEC")]
        base_spec: Option<PathBuf>,
        /// Count enum values added to responses as minor changes, not breaking ones: right while
        /// the SDKs accept unknown enum values, as generated ones do.
        #[arg(long, action = ArgAction::Set, default_value_t = true, env = "PERSEID_RELAX_ENUM_ADDITIONS")]
        relax_enum_additions: bool,
        /// Enable auto-merge (squash) on each pull request, labelled `perseid:auto-release` so
        /// the scaffolded sdk-release.yml auto-merges the release PR it leads to.
        #[arg(long, requires = "pr", env = "PERSEID_AUTO_MERGE")]
        auto_merge: bool,
        /// Workflow files to run on each update branch once its pull request is opened or updated
        /// in this repository: pushes made with the default `GITHUB_TOKEN` start none.
        #[arg(long, requires = "pr", num_args = 1.., value_delimiter = ' ', env = "PERSEID_DISPATCH")]
        dispatch: Vec<String>,
        /// Skip formatters.
        #[arg(long)]
        no_format: bool,
    },
    /// Write the targets perseid.toml lists besides the SDKs, `[targets.<name>]`: for `docs`, the
    /// spec and the docs data, in the folder `path` of its repository; for a pack, the program it
    /// renders around an SDK.
    Targets {
        /// Targets to write (default: all).
        names: Vec<String>,
        /// OpenAPI document to read, path or URL, over `spec` of perseid.toml.
        #[arg(long)]
        spec: Option<String>,
        /// Write each target under this directory, in a folder named after it, without checking
        /// out its repository.
        #[arg(long, conflicts_with = "pr", required_unless_present = "pr")]
        out: Option<PathBuf>,
        /// Fail if the targets under `--out` are out of date, without writing anything.
        #[arg(long, requires = "out")]
        check: bool,
        /// Skip the formatters of pack targets.
        #[arg(long)]
        no_format: bool,
        /// Open or update the pull request of each target whose `after` holds, from
        /// `perseid/targets/<name>`, as `generate --pr` does after the SDK pull requests.
        #[arg(long)]
        pr: bool,
        /// Release size the pull request asks for (default: sized against the spec the target
        /// holds).
        #[arg(
            long,
            value_enum,
            requires = "pr",
            default_value = "auto",
            env = "PERSEID_BUMP"
        )]
        bump: Bump,
        /// Count enum values added to responses as minor changes, as `generate` does.
        #[arg(long, action = ArgAction::Set, default_value_t = true, env = "PERSEID_RELAX_ENUM_ADDITIONS")]
        relax_enum_additions: bool,
        /// Enable auto-merge (squash) on each pull request.
        #[arg(long, requires = "pr", env = "PERSEID_AUTO_MERGE")]
        auto_merge: bool,
    },
    /// Set up the repositories perseid.toml names, once you agree: install the perseid App on
    /// them, and commit the release workflow to each SDK repository.
    Sync {
        #[command(flatten)]
        apply: Apply,
    },
    /// Create or reuse a GitHub App of your own that opens the SDK pull requests, install it and
    /// store its credentials, once you agree: the alternative to the hosted perseid App.
    App {
        #[command(flatten)]
        apply: Apply,
    },
    /// In the repository holding the spec: push it to the SDKs repository on each release or change.
    Connect {
        /// The SDKs repository, `owner/name`, holding perseid.toml.
        #[arg(value_name = "OWNER/SDKS_REPO")]
        hub: String,
        /// The OpenAPI document, relative to this repository (default: detected).
        #[arg(long)]
        spec: Option<String>,
        /// Command writing the spec in CI, when it isn't committed.
        #[arg(long)]
        build: Option<String>,
        /// When to push the spec (default: `release` when the repository has releases, else `change`).
        #[arg(long, value_enum)]
        on: Option<PushOn>,
        /// Tags pushing the spec with `--on tag`, as a GitHub Actions glob (default: `v*`).
        #[arg(long)]
        tags: Option<String>,
        /// How the workflow authenticates to the SDKs repository (default: the GitHub App of the
        /// SDKs repository, when `perseid app` set one up, else a token).
        #[arg(long, value_enum)]
        auth: Option<Auth>,
        /// Leave this repository's name out of what the SDKs repository records.
        #[arg(long)]
        private: bool,
        #[command(flatten)]
        apply: Apply,
    },
    /// Commit the spec to the SDKs repository, unless it holds a newer one: what perseid-push.yml
    /// runs, authenticating with the token in `PERSEID_SDKS_TOKEN` when set.
    #[command(hide = true)]
    PushSpec {
        /// The OpenAPI document, relative to this directory.
        spec: String,
        /// The SDKs repository, `owner/name`.
        #[arg(long)]
        to: String,
        /// Leave this repository's name out of what the SDKs repository records.
        #[arg(long)]
        private: bool,
    },
    /// Check the GitHub setup without changing it: pending changes, last spec, pull requests, runs.
    Status,
    /// Copy the built-in templates and runtime of a language into `.perseid/` to customize them.
    Eject {
        /// The language whose templates and runtime to copy.
        #[arg(value_parser = LANGUAGES)]
        language: String,
        /// Only these files, as `--list` prints them (default: every file).
        files: Vec<String>,
        /// List the files, without copying any.
        #[arg(long)]
        list: bool,
    },
    /// Print the API model templates receive, as JSON: for `language`, with its own `exclude`,
    /// `methods` and reserved names applied.
    Inspect {
        #[arg(value_parser = LANGUAGES)]
        language: Option<String>,
        /// OpenAPI document to read, path or URL, over `spec` of perseid.toml.
        #[arg(long)]
        spec: Option<String>,
    },
    /// Print how each SDK installs, sets up and names its client, and names and calls every
    /// operation (signature, sample), as JSON for docs sites showing the reader's SDK language.
    DocsData {
        /// Languages to describe (default: all configured).
        #[arg(value_parser = LANGUAGES)]
        languages: Vec<String>,
        /// OpenAPI document to read, path or URL, over `spec` of perseid.toml.
        #[arg(long)]
        spec: Option<String>,
        /// Write the JSON to this file instead.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// The pinned formatters the SDKs need, and oasdiff sizing their releases.
    Tools {
        #[command(subcommand)]
        command: Tools,
    },
    /// Print the JSON Schema of perseid.toml.
    #[command(hide = true)]
    Schema,
    /// Write schema-derived sample JSON of every model, with the type name each language
    /// generates, for round-trip tests of the SDKs.
    #[command(hide = true)]
    Samples {
        /// OpenAPI document to read, path or URL, over `spec` of perseid.toml (which is then
        /// optional).
        #[arg(long)]
        spec: Option<String>,
        /// The JSON file to write.
        #[arg(long)]
        out: PathBuf,
    },
}

#[derive(Subcommand)]
enum Tools {
    /// Print the tools the SDKs need, with their pinned versions.
    List {
        /// Languages to list the tools of (default: all configured).
        #[arg(value_parser = LANGUAGES)]
        languages: Vec<String>,
        /// Also append `languages`, and the `owner` and `repositories` a GitHub App token for
        /// the pull requests covers, to `$GITHUB_OUTPUT`.
        #[arg(long)]
        github_output: bool,
    },
    /// Download the tools the SDKs need, at their pinned versions, and check each runs.
    Install {
        /// Languages to install the tools of (default: all configured).
        #[arg(value_parser = LANGUAGES)]
        languages: Vec<String>,
        /// Where to install them (default: next to perseid).
        #[arg(long)]
        dir: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    let config_path = Config::locate(&cli.config);
    match cli.command {
        Command::Init {
            sdks,
            repo,
            spec,
            name,
            base_url,
            license,
            from,
        } => {
            let root = cli.config.parent().map(|p| cwd.join(p)).unwrap_or(cwd);
            let init = init::Init {
                spec,
                name,
                sdks,
                repo,
                base_url,
                license,
                from,
            };
            init::run(init, &root)?;
        }
        Command::Sync { apply } => return perseid::github::sync(&config_path, &apply.options()),
        Command::App { apply } => return perseid::github::app(&config_path, &apply.options()),
        Command::Connect {
            hub,
            spec,
            build,
            on,
            tags,
            auth,
            private,
            apply,
        } => {
            let connect = perseid::github::Connect {
                hub,
                spec,
                build,
                on,
                tags,
                auth,
                private,
            };
            return perseid::github::connect(&cwd, connect, &apply.options());
        }
        Command::PushSpec { spec, to, private } => {
            let push = perseid::github::PushSpec { spec, to, private };
            return perseid::github::push_spec(&cwd, push);
        }
        Command::Status => return perseid::github::status(&config_path),
        Command::Generate {
            languages,
            spec,
            check,
            out,
            pr,
            bump,
            base_spec,
            relax_enum_additions,
            auto_merge,
            dispatch,
            no_format,
        } => {
            let (mut config, root) = Config::load(&config_path)?;
            let location = spec.unwrap_or_else(|| config.spec.clone());
            let spec = generate::load_spec(&config, &root, Some(&location))?;
            config.default_to_spec_server(&spec);
            let options = Options {
                check,
                format: !no_format,
            };
            let sdks = config.sdks(&languages)?;
            let out = out.map(|out| cwd.join(out));
            let mut files = vec![];
            let mut checkouts = BTreeMap::new();
            let mut dirs = Vec::new();
            for sdk in &sdks {
                dirs.push(match (&out, sdk.remote()) {
                    (Some(out), _) if sdk.path == "." => out.join(sdk.language),
                    (Some(out), _) => out.join(&sdk.path),
                    (None, Some(repo)) => {
                        let checkout = pr::checkout(repo, &root, pr)?;
                        checkouts.insert(repo.to_owned(), checkout.clone());
                        checkout.join(&sdk.path)
                    }
                    (None, None) => root.join(&sdk.path),
                });
            }
            if let Some(twice) = dirs
                .iter()
                .find(|d| dirs.iter().filter(|o| o == d).count() > 1)
            {
                bail!(
                    "several SDKs would be written to {}: set a distinct `path` for each",
                    twice.display()
                );
            }
            if !check {
                for (sdk, dir) in sdks.iter().zip(&dirs) {
                    files.extend(scaffold::bootstrap(&config, &root, sdk, dir, &spec)?);
                }
                for (repo, checkout) in checkouts.iter().filter(|_| config.release != Some(false)) {
                    let held: Vec<&config::Sdk> =
                        sdks.iter().filter(|s| s.remote() == Some(repo)).collect();
                    files.extend(perseid::github::write_release_files(
                        &config, &held, checkout,
                    )?);
                }
            }
            let results: Vec<_> = std::thread::scope(|scope| {
                let handles: Vec<_> = sdks
                    .iter()
                    .zip(&dirs)
                    .map(|(sdk, dir)| {
                        // Templates recurse through nested types (a union in a list in a
                        // nullable...): large specs need more than the default 2 MiB of stack.
                        std::thread::Builder::new()
                            .stack_size(64 << 20)
                            .spawn_scoped(scope, || {
                                generate::sdk(&config, &root, sdk, dir, &spec, &options)
                            })
                            .expect("spawning a generation thread")
                    })
                    .collect();
                handles
                    .into_iter()
                    .map(|h| h.join().expect("generation thread panicked"))
                    .collect()
            });
            if !check {
                checkouts.values().try_for_each(|c| pr::record(c))?;
            }
            let mut changes = BTreeMap::new();
            for (sdk, result) in sdks.iter().zip(results) {
                changes.insert(
                    sdk.language.to_owned(),
                    result.with_context(|| format!("generating {}", sdk.language))?,
                );
            }
            let places = sdks
                .iter()
                .zip(&dirs)
                .map(|(sdk, dir)| (sdk.language.to_owned(), shown(dir, &cwd)))
                .collect();
            println!("{}", generate::summary(&changes, &places));
            let changed = changes.values().any(|c| !c.is_empty());
            if check && changed {
                eprintln!("\nSDKs are out of date, run `perseid generate`");
                return Ok(ExitCode::FAILURE);
            }
            if pr {
                let base = base_spec.map(|b| cwd.join(b));
                let asked = bump;
                let (bump, changelog) = match bump {
                    Bump::Auto => sizing::size(&sizing::Comparison {
                        root: &root,
                        spec: &location,
                        base: base.as_deref(),
                        relax_enum_additions,
                    })?,
                    bump => (bump, Default::default()),
                };
                let request = Request {
                    bump,
                    changelog,
                    origin: pr::origin(&config, &root, &spec),
                    auto_merge,
                    dispatch: dispatch
                        .iter()
                        .flat_map(|d| d.split_whitespace())
                        .map(str::to_owned)
                        .collect(),
                    hub: pr::toplevel(&root).ok(),
                };
                if let config::Source::File(file) = config.source() {
                    files.push(root.join(file));
                }
                let names: Vec<_> = sdks.iter().map(|s| s.language.to_owned()).collect();
                let github = pr::Client::default();
                deliver(&github, &names, &dirs, &files, &spec, changes, &request)?;
                let targets = config.targets(&[])?;
                let request = targets::Request {
                    bump: asked,
                    relax_enum_additions,
                    auto_merge,
                    format: !no_format,
                };
                targets::deliver(&github, &config, &root, &targets, &spec, &request)?;
            }
        }
        Command::Targets {
            names,
            spec,
            out,
            check,
            no_format,
            pr: _,
            bump,
            relax_enum_additions,
            auto_merge,
        } => {
            let (config, root) = Config::load(&config_path)?;
            let spec = generate::load_spec(&config, &root, spec.as_deref())?;
            let targets = config.targets(&names)?;
            if targets.is_empty() {
                println!("no [targets] in {}", config::FILE);
                return Ok(ExitCode::SUCCESS);
            }
            match out {
                Some(out) => {
                    let out = cwd.join(out);
                    let options = Options {
                        check,
                        format: !no_format,
                    };
                    let changes =
                        targets::preview(&config, &root, &targets, &spec, &out, &options)?;
                    let places = (targets.iter())
                        .map(|t| (t.name.clone(), shown(&out.join(&t.name), &cwd)))
                        .collect();
                    println!("{}", generate::summary(&changes, &places));
                    if check && changes.values().any(|c| !c.is_empty()) {
                        eprintln!("\ntargets are out of date, run `perseid targets --out`");
                        return Ok(ExitCode::FAILURE);
                    }
                }
                None => {
                    let request = targets::Request {
                        bump,
                        relax_enum_additions,
                        auto_merge,
                        format: !no_format,
                    };
                    let github = pr::Client::default();
                    targets::deliver(&github, &config, &root, &targets, &spec, &request)?;
                }
            }
        }
        Command::Eject {
            language,
            files,
            list,
        } => {
            let (config, root) = Config::load(&config_path)?;
            if list {
                for path in perseid::assets::ejectable(&language) {
                    println!("{path}");
                }
                return Ok(ExitCode::SUCCESS);
            }
            let dir = config.overrides_dir(&root);
            for path in perseid::assets::eject(&language, &files, &dir)? {
                println!("+ {}", path.strip_prefix(&root).unwrap_or(&path).display());
            }
            println!(
                "\nEdit these files, then delete the ones you didn't change so they keep receiving upstream updates."
            );
        }
        Command::Schema => print!("{}", config::json_schema()),
        Command::Samples { spec, out } => {
            let samples = match Config::load(&config_path) {
                Ok((config, root)) => {
                    let location = spec.unwrap_or_else(|| config.spec.clone());
                    let text = generate::load_spec(&config, &root, Some(&location))?;
                    let targets: Vec<_> = config
                        .sdks(&[])?
                        .iter()
                        .map(|sdk| (sdk.language, config.filters_for(sdk)))
                        .collect();
                    perseid::samples::run(&text, &config.filters(), &targets)?
                }
                Err(error) => {
                    let Some(location) = spec else {
                        return Err(error);
                    };
                    let (text, base, targets) = perseid::samples::without_config(&location, &cwd)?;
                    perseid::samples::run(&text, &base, &targets)?
                }
            };
            std::fs::write(
                cwd.join(&out),
                serde_json::to_string_pretty(&samples)? + "\n",
            )
            .with_context(|| format!("writing {}", out.display()))?;
            let models = samples.as_object().map_or(0, |m| m.len());
            println!("wrote the samples of {models} models to {}", out.display());
        }
        Command::Tools { command } => tools_command(&config_path, &cwd, command)?,
        Command::DocsData {
            languages,
            spec,
            out,
        } => {
            let (mut config, root) = Config::load(&config_path)?;
            let spec = generate::load_spec(&config, &root, spec.as_deref())?;
            config.default_to_spec_server(&spec);
            let sdks = config.sdks(&languages)?;
            let data = perseid::docs_data::run(&config, &root, &sdks, &spec, &BTreeMap::new())?;
            let text = serde_json::to_string_pretty(&data)? + "\n";
            match out {
                Some(out) => std::fs::write(cwd.join(&out), text)
                    .with_context(|| format!("writing {}", out.display()))?,
                None => print!("{text}"),
            }
        }
        Command::Inspect { language, spec } => {
            let (config, root) = Config::load(&config_path)?;
            let spec = generate::load_spec(&config, &root, spec.as_deref())?;
            let filters = match language {
                Some(language) => {
                    let sdk = config.sdks(&[language])?.remove(0);
                    config.filters_for(&sdk)
                }
                None => config.filters(),
            };
            println!("{}", perseid::inspect(&spec, &filters)?);
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn tools_command(config_path: &Path, cwd: &Path, command: Tools) -> Result<()> {
    let configured = |config: &Config| -> Result<Vec<String>> {
        let sdks = config.sdks(&[])?;
        Ok(sdks.iter().map(|s| s.language.to_owned()).collect())
    };
    match command {
        Tools::List {
            languages,
            github_output,
        } => {
            let loaded = (languages.is_empty() || github_output)
                .then(|| Config::load(config_path))
                .transpose()?;
            let languages = match &loaded {
                Some((config, _)) if languages.is_empty() => configured(config)?,
                _ => languages,
            };
            for tool in tools::needed(languages.iter().map(String::as_str)) {
                println!("{} {}", tool.name(), tool.version());
            }
            if let Some((config, _)) = loaded.filter(|_| github_output) {
                let hub = config
                    .home
                    .repo()
                    .map(str::to_owned)
                    .or_else(|| std::env::var("GITHUB_REPOSITORY").ok());
                let written = targets::repos(&config.targets(&[])?);
                let (owner, repositories) =
                    tools::app_scope(hub.as_deref(), &config.sdks(&[])?, &written);
                tools::github_output(&[
                    ("languages", languages.join(" ")),
                    ("owner", owner.unwrap_or_default()),
                    ("repositories", repositories.join(",")),
                ])?;
            }
        }
        Tools::Install { languages, dir } => {
            let languages = match languages.is_empty() {
                true => configured(&Config::load(config_path)?.0)?,
                false => languages,
            };
            let dir = match dir {
                Some(dir) => cwd.join(dir),
                None => tools::default_dir()?,
            };
            for tool in tools::needed(languages.iter().map(String::as_str)) {
                tools::install(tool, &dir)?;
            }
        }
    }
    Ok(())
}

/// `dir` relative to `cwd` when under it.
fn shown(dir: &Path, cwd: &Path) -> String {
    match dir.strip_prefix(cwd) {
        Ok(relative) if relative.as_os_str().is_empty() => ".".to_owned(),
        Ok(relative) => relative.display().to_string(),
        Err(_) => dir.display().to_string(),
    }
}

/// SDK directories and other files (spec, skeleton) to commit, relative to their repository.
#[derive(Default)]
struct Delivery {
    /// The SDKs, with their directory.
    sdks: Vec<(String, String)>,
    files: Vec<String>,
    changes: BTreeMap<String, Vec<generate::Change>>,
}

impl Delivery {
    /// The files of `files` under `dir`.
    fn files_under(&self, dir: &str) -> Vec<String> {
        self.files
            .iter()
            .filter(|f| f.starts_with(&format!("{dir}/")))
            .cloned()
            .collect()
    }

    /// Whether its update can be split in one pull request per SDK: it holds several, none at
    /// its root.
    fn splittable(&self) -> bool {
        self.sdks.len() > 1 && self.sdks.iter().all(|(_, dir)| dir != ".")
    }
}

/// What `--pr` asks of each pull request.
struct Request {
    bump: Bump,
    /// The API changes, summed up in the SDK changelogs.
    changelog: perseid::changelog::Changelog,
    /// Where the spec comes from.
    origin: Option<String>,
    auto_merge: bool,
    /// Workflows run on the update branch of `hub`'s pull request.
    dispatch: Vec<String>,
    /// The repository holding perseid.toml.
    hub: Option<PathBuf>,
}

fn deliver(
    github: &pr::Client,
    names: &[String],
    dirs: &[PathBuf],
    files: &[PathBuf],
    spec: &str,
    mut changes: BTreeMap<String, Vec<generate::Change>>,
    request: &Request,
) -> Result<()> {
    let mut repos: BTreeMap<PathBuf, Delivery> = BTreeMap::new();
    for (name, dir) in names.iter().zip(dirs) {
        let top = pr::toplevel(dir)?;
        let path = std::path::absolute(dir)?;
        let relative = path
            .strip_prefix(&top)
            .context("SDK outside its repository")?
            .to_str()
            .filter(|p| !p.is_empty())
            .unwrap_or(".");
        let delivery = repos.entry(top).or_default();
        delivery.sdks.push((name.clone(), relative.to_owned()));
        if let Some(changed) = changes.remove(name) {
            delivery.changes.insert(name.clone(), changed);
        }
    }
    for file in files {
        if let Ok(path) = std::path::absolute(file)
            && let Some(parent) = path.parent()
            && let Ok(top) = pr::toplevel(parent)
            && let Some(delivery) = repos.get_mut(&top)
            && let Ok(relative) = path.strip_prefix(&top)
        {
            delivery.files.push(relative.to_string_lossy().into_owned());
        }
    }
    let api = perseid::targets::api_name(spec);
    let credit = format!(
        "Generated by [perseid](https://github.com/meteroid-oss/perseid){}.",
        request
            .origin
            .as_ref()
            .map_or_else(String::new, |o| format!(" from {o}"))
    );
    let credit = &credit;
    let describe = |changes: BTreeMap<String, Vec<generate::Change>>| {
        move |listed: &str| {
            let summary = generate::summary(&changes, &BTreeMap::new());
            let listed = match listed {
                "" => String::new(),
                listed => format!(
                    "{listed}

"
                ),
            };
            format!(
                "```
{summary}
```

{listed}{credit}"
            )
        }
    };
    for (repo, mut delivery) in repos {
        let open = |update: &pr::Update, subject: &str, changes| {
            pr::open(
                github,
                &repo,
                update,
                request.bump,
                subject,
                &request.changelog,
                describe(changes),
            )
        };
        let mut pulls = Vec::new();
        let dirs: Vec<String> = delivery.sdks.iter().map(|(_, dir)| dir.clone()).collect();
        let whole = pr::Update {
            branch: pr::BRANCH,
            paths: &dirs,
            files: &delivery.files,
            shared: &[],
            owned: &[],
        };
        // One pull request per SDK while the update is too large for release-please, and until
        // those opened are merged.
        let split = delivery.splittable()
            && (pr::splitting(github, &repo)?
                || pr::changed_files(&repo, &whole)? > pr::split_above());
        if split {
            let mut changes = std::mem::take(&mut delivery.changes);
            let shared: Vec<String> = delivery
                .files
                .iter()
                .filter(|f| !dirs.iter().any(|d| f.starts_with(&format!("{d}/"))))
                .cloned()
                .collect();
            for (name, dir) in &delivery.sdks {
                let branch = pr::branch_of(name);
                let update = pr::Update {
                    branch: &branch,
                    paths: std::slice::from_ref(dir),
                    files: &delivery.files_under(dir),
                    shared: &shared,
                    owned: &[],
                };
                let changes = changes
                    .remove(name)
                    .map(|c| BTreeMap::from([(name.clone(), c)]))
                    .unwrap_or_default();
                let subject = format!("update the {name} SDK to {api}");
                match open(&update, &subject, changes)? {
                    Some(pull) => pulls.push(pull),
                    None => {
                        println!("{}: nothing to update in {dir}", repo.display());
                        pr::close_stale(github, &repo, &branch)?;
                    }
                }
            }
            if pulls.is_empty() {
                // The spec or release files alone, changed while every SDK stayed the same.
                let update = pr::Update {
                    branch: pr::BRANCH,
                    paths: &[],
                    files: &shared,
                    shared: &[],
                    owned: &[],
                };
                pulls.extend(open(&update, &format!("update SDKs to {api}"), changes)?);
            } else {
                pr::close_superseded(github, &repo, &pulls)?;
            }
        } else {
            let changes = std::mem::take(&mut delivery.changes);
            pulls.extend(open(&whole, &format!("update SDKs to {api}"), changes)?);
        }
        if pulls.is_empty() {
            println!("{}: nothing to update", repo.display());
        }
        for pull in &pulls {
            println!("{}", pull.url);
            if request.auto_merge {
                pr::auto_merge(github, pull)?;
            }
            if request.hub.as_ref() == Some(&repo) {
                pr::dispatch(github, pull, &request.dispatch)?;
            }
        }
    }
    Ok(())
}

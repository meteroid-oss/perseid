use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    process::ExitCode,
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use perseid::{
    config::{self, Config, LANGUAGES},
    generate::{self, Options},
    github::{Auth, PushOn},
    init,
    pr::{self, Bump},
    scaffold,
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
    /// Write perseid.toml, in the repository that will hold the SDKs: which SDKs, where they live.
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
        /// temporary worktree, and open a pull request (needs `gh`).
        #[arg(long)]
        pr: bool,
        /// Release size the pull request asks for, as its conventional-commit type.
        #[arg(long, value_enum, requires = "pr", default_value = "minor")]
        bump: Bump,
        /// Markdown file appended to the pull request description, e.g. an API changelog.
        #[arg(long, requires = "pr")]
        notes: Option<PathBuf>,
        /// Skip formatters.
        #[arg(long)]
        no_format: bool,
    },
    /// Set up GitHub for the SDKs perseid.toml describes: repositories, the App, workflows, releases.
    Setup {
        #[command(flatten)]
        apply: Apply,
    },
    /// In the repository holding the spec: push it to the SDKs repository on each release or change.
    Connect {
        /// The SDKs repository, `owner/name`, holding perseid.toml.
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
        /// How the workflow authenticates to the SDKs repository (default: a deploy key).
        #[arg(long, value_enum)]
        auth: Option<Auth>,
        /// Leave this repository's name out of what the SDKs repository records.
        #[arg(long)]
        private: bool,
        #[command(flatten)]
        apply: Apply,
    },
    /// Commit the spec to the SDKs repository, unless it holds a newer one: what perseid-push.yml
    /// runs, authenticating with the deploy key in `PERSEID_SDKS_DEPLOY_KEY` when set.
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
        #[arg(value_parser = LANGUAGES)]
        language: String,
    },
    /// Print the API model templates receive, as JSON.
    Inspect,
    /// Print the JSON Schema of perseid.toml.
    #[command(hide = true)]
    Schema,
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
    match cli.command {
        Command::Init {
            sdks,
            repo,
            spec,
            name,
            base_url,
        } => {
            let root = cli.config.parent().map(|p| cwd.join(p)).unwrap_or(cwd);
            let init = init::Init {
                spec,
                name,
                sdks,
                repo,
                base_url,
            };
            init::run(init, &root)?;
        }
        Command::Setup { apply } => return perseid::github::setup(&cli.config, &apply.options()),
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
        Command::Status => return perseid::github::status(&cli.config),
        Command::Generate {
            languages,
            spec,
            check,
            out,
            pr,
            bump,
            notes,
            no_format,
        } => {
            let (config, root) = Config::load(&cli.config)?;
            let spec = generate::load_spec(&config, &root, spec.as_deref())?;
            let options = Options {
                check,
                format: !no_format,
            };
            let sdks = config.sdks(&languages)?;
            let out = out.map(|out| cwd.join(out));
            let mut files = vec![];
            let mut checkouts = BTreeSet::new();
            let mut dirs = Vec::new();
            for sdk in &sdks {
                dirs.push(match (&out, sdk.remote()) {
                    (Some(out), _) if sdk.path == "." => out.join(sdk.language),
                    (Some(out), _) => out.join(&sdk.path),
                    (None, Some(repo)) => {
                        let checkout = pr::checkout(repo, &root, pr)?;
                        checkouts.insert(checkout.clone());
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
                    files.extend(scaffold::bootstrap(&config, sdk, dir)?);
                }
            }
            let results: Vec<_> = std::thread::scope(|scope| {
                let handles: Vec<_> = sdks
                    .iter()
                    .zip(&dirs)
                    .map(|(sdk, dir)| {
                        scope.spawn(|| generate::sdk(&config, &root, sdk, dir, &spec, &options))
                    })
                    .collect();
                handles
                    .into_iter()
                    .map(|h| h.join().expect("generation thread panicked"))
                    .collect()
            });
            if !check {
                checkouts.iter().try_for_each(|c| pr::record(c))?;
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
                let notes = notes
                    .map(|path| std::fs::read_to_string(&path).context("reading --notes"))
                    .transpose()?;
                let origin = pr::origin(&config, &root, &spec);
                if let config::Source::File(file) = config.source() {
                    files.push(root.join(file));
                }
                deliver(&dirs, &files, &spec, &changes, bump, notes, origin)?;
            }
        }
        Command::Eject { language } => {
            let (config, root) = Config::load(&cli.config)?;
            let dir = config.overrides_dir(&root);
            for path in perseid::assets::eject(&language, &dir)? {
                println!("+ {}", path.strip_prefix(&root).unwrap_or(&path).display());
            }
            println!(
                "\nEdit these files, then delete the ones you didn't change so they keep receiving upstream updates."
            );
        }
        Command::Schema => print!("{}", config::json_schema()),
        Command::Inspect => {
            let (config, root) = Config::load(&cli.config)?;
            let spec = generate::load_spec(&config, &root, None)?;
            println!("{}", perseid::inspect(&spec, &config)?);
        }
    }
    Ok(ExitCode::SUCCESS)
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
    dirs: Vec<String>,
    files: Vec<String>,
}

fn deliver(
    dirs: &[PathBuf],
    files: &[PathBuf],
    spec: &str,
    changes: &BTreeMap<String, Vec<generate::Change>>,
    bump: Bump,
    notes: Option<String>,
    origin: Option<String>,
) -> Result<()> {
    let spec: serde_json::Value = serde_json::from_str(spec)?;
    let mut repos: BTreeMap<PathBuf, Delivery> = BTreeMap::new();
    for dir in dirs {
        let top = pr::toplevel(dir)?;
        let path = std::path::absolute(dir)?;
        let relative = path
            .strip_prefix(&top)
            .context("SDK outside its repository")?
            .to_str()
            .filter(|p| !p.is_empty())
            .unwrap_or(".");
        repos.entry(top).or_default().dirs.push(relative.to_owned());
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
    let subject = format!(
        "update SDKs to {} {}",
        spec["info"]["title"].as_str().unwrap_or("the API"),
        spec["info"]["version"].as_str().unwrap_or_default()
    );
    let mut body = format!(
        "Generated by [perseid](https://github.com/meteroid-oss/perseid){}.\n\n```\n{}\n```",
        origin.map_or_else(String::new, |o| format!(" from {o}")),
        generate::summary(changes, &BTreeMap::new())
    );
    if let Some(notes) = notes {
        body += &format!("\n\n{}", notes.trim());
    }
    for (repo, delivery) in repos {
        match pr::open(
            &repo,
            &delivery.dirs,
            &delivery.files,
            bump,
            subject.trim(),
            &body,
        )? {
            Some(url) => println!("{url}"),
            None => println!("{}: nothing to update", repo.display()),
        }
    }
    Ok(())
}

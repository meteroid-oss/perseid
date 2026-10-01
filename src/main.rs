use std::{collections::BTreeMap, path::PathBuf, process::ExitCode};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use perseid::{
    config::{self, Config, LANGUAGES},
    generate::{self, Options},
    github::PushOn,
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
        /// Leave this repository's name out of what the SDKs repository records.
        #[arg(long)]
        private: bool,
        #[command(flatten)]
        apply: Apply,
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
            private,
            apply,
        } => {
            let connect = perseid::github::Connect {
                hub,
                spec,
                build,
                on,
                tags,
                private,
            };
            return perseid::github::connect(&cwd, connect, &apply.options());
        }
        Command::Status => return perseid::github::status(&cli.config),
        Command::Generate {
            languages,
            spec,
            check,
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
            let mut files = vec![];
            let dirs = sdks
                .iter()
                .map(|sdk| {
                    let repo = match &sdk.repo {
                        Some(repo) => pr::checkout(repo, &root)?,
                        None => root.clone(),
                    };
                    if !check {
                        files.extend(scaffold::bootstrap(&config, sdk, &repo)?);
                    }
                    Ok(repo.join(&sdk.path))
                })
                .collect::<Result<Vec<_>>>()?;
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
            let mut changes = BTreeMap::new();
            for (sdk, result) in sdks.iter().zip(results) {
                changes.insert(
                    sdk.language.to_owned(),
                    result.with_context(|| format!("generating {}", sdk.language))?,
                );
            }
            println!("{}", generate::summary(&changes));
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
        generate::summary(changes)
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

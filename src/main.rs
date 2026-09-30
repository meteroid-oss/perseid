use std::{collections::BTreeMap, path::PathBuf, process::ExitCode};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use perseid::{
    config::{self, Config, LANGUAGES},
    generate::{self, Options},
    init,
    pr::{self, Bump},
};

/// OpenAPI in, idiomatic SDKs out. Rust, TypeScript, Python, Go and Java.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Path to the project file.
    #[arg(short, long, global = true, default_value = config::FILE)]
    config: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create perseid.toml and SDK package skeletons (re-run to scaffold newly added languages).
    Init {
        /// Languages to scaffold (default: all).
        #[arg(value_parser = LANGUAGES)]
        languages: Vec<String>,
        /// OpenAPI document, path or URL (default: detected).
        #[arg(long)]
        spec: Option<String>,
        /// Client name (default: derived from the spec title).
        #[arg(long)]
        name: Option<String>,
        /// API base URL (default: first server of the spec).
        #[arg(long)]
        base_url: Option<String>,
        /// Skip the release-please files and the SDK release workflow.
        #[arg(long)]
        no_release: bool,
        /// Set up GitHub: SDK repositories, a GitHub App and the workflow opening SDK pull requests.
        #[arg(long)]
        github: bool,
        /// Account owning the SDK repositories and the App (default: the spec repository's).
        #[arg(long, requires = "github")]
        github_owner: Option<String>,
        /// Answer every question with its default.
        #[arg(long, short)]
        yes: bool,
        /// Print URLs instead of opening them in a browser.
        #[arg(long)]
        no_browser: bool,
        /// Commit the GitHub workflow to the default branch instead of opening a pull request.
        #[arg(long, requires = "github")]
        push: bool,
    },
    /// Generate SDKs.
    Generate {
        /// Languages to generate (default: all configured).
        #[arg(value_parser = LANGUAGES)]
        languages: Vec<String>,
        /// Fail if generated files are out of date, without writing anything.
        #[arg(long, conflicts_with = "pr")]
        check: bool,
        /// Commit the result to the `perseid/update` branch and open a pull request (needs `gh`).
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
    /// Copy the built-in templates and runtime of a language into `.perseid/` to customize them.
    Eject {
        #[arg(value_parser = LANGUAGES)]
        language: String,
    },
    /// Print the API model templates receive, as JSON.
    Inspect,
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
            languages,
            spec,
            name,
            base_url,
            no_release,
            github,
            github_owner,
            yes,
            no_browser,
            push,
        } => {
            let root = cli.config.parent().map(|p| cwd.join(p)).unwrap_or(cwd);
            let options = perseid::github::Options {
                owner: github_owner,
                yes,
                browser: !no_browser,
                push,
            };
            let init = init::Init {
                languages,
                spec,
                name,
                base_url,
                release: !no_release,
            };
            let ui = perseid::github::Ui::new(&options);
            let created = match github {
                true => init::run_with(init, &root, |path| {
                    perseid::github::layout(path, &ui, options.owner.as_deref())
                })?,
                false => init::run(init, &root)?,
            };
            for path in created {
                println!("+ {}", path.strip_prefix(&root).unwrap_or(&path).display());
            }
            let config_path = root.join(config::FILE);
            let offered = !github && !yes && perseid::github::offered(&root);
            if offered && ui.confirm("Set up GitHub automation?", true)? {
                perseid::github::layout(&config_path, &ui, None)?;
                perseid::github::setup(&config_path, &options)?;
            } else if github {
                perseid::github::setup(&config_path, &options)?;
            } else {
                println!("\nnext: perseid generate");
            }
        }
        Command::Generate {
            languages,
            check,
            pr,
            bump,
            notes,
            no_format,
        } => {
            let (config, root) = Config::load(&cli.config)?;
            let spec = generate::load_spec(&config, &root)?;
            let info: serde_json::Value = serde_json::from_str(&spec)?;
            let options = Options {
                check,
                format: !no_format,
            };
            let sdks = config.sdks(&languages)?;
            let dirs = sdks
                .iter()
                .map(|sdk| match sdk.repo {
                    Some(repo) => {
                        let checkout = pr::checkout(repo, &root)?;
                        if !check {
                            init::bootstrap(&config, sdk, &checkout)?;
                        }
                        Ok(checkout.join(&sdk.path))
                    }
                    None => Ok(root.join(&sdk.path)),
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
                deliver(&config, &root, &dirs, &info, &changes, bump, notes)?;
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
        Command::Inspect => {
            let (config, root) = Config::load(&cli.config)?;
            let spec = generate::load_spec(&config, &root)?;
            println!("{}", perseid::inspect(&spec, &config)?);
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn deliver(
    config: &Config,
    root: &std::path::Path,
    dirs: &[PathBuf],
    spec: &serde_json::Value,
    changes: &BTreeMap<String, Vec<generate::Change>>,
    bump: Bump,
    notes: Option<String>,
) -> Result<()> {
    let mut repos: BTreeMap<PathBuf, Vec<String>> = BTreeMap::new();
    for dir in dirs {
        let top = pr::toplevel(dir)?;
        let path = std::path::absolute(dir)?;
        let relative = path
            .strip_prefix(&top)
            .context("SDK outside its repository")?;
        repos
            .entry(top)
            .or_default()
            .push(relative.to_str().unwrap_or(".").to_owned());
    }
    if let Ok(top) = pr::toplevel(root)
        && let Some(paths) = repos.get_mut(&top)
        && let Ok(spec_path) = std::path::absolute(root.join(&config.spec))
        && let Ok(relative) = spec_path.strip_prefix(&top)
    {
        paths.push(relative.to_string_lossy().into_owned());
    }
    let subject = format!(
        "update SDKs to {} {}",
        spec["info"]["title"].as_str().unwrap_or("the API"),
        spec["info"]["version"].as_str().unwrap_or_default()
    );
    let mut body = format!(
        "Generated by [perseid](https://github.com/meteroid-oss/perseid).\n\n```\n{}\n```",
        generate::summary(changes)
    );
    if let Some(notes) = notes {
        body += &format!("\n\n{}", notes.trim());
    }
    for (repo, mut paths) in repos {
        paths
            .iter_mut()
            .filter(|p| p.is_empty())
            .for_each(|p| *p = ".".into());
        match pr::open(&repo, &paths, bump, subject.trim(), &body)? {
            Some(url) => println!("{url}"),
            None => println!("{}: nothing to update", repo.display()),
        }
    }
    Ok(())
}

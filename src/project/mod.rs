//! Native project lifecycle. No interpreter or hosted service is required.
pub mod assets;
pub mod config;
mod github;
pub mod init;
pub mod io;
pub mod prepare;
mod release;
mod source;
mod workspace;
use anyhow::{Result, ensure};
use assets::Assets;
use clap::{Args, Subcommand};
use config::Project;
use std::{fs, path::PathBuf};

#[derive(Clone, Debug, Args)]
pub struct Selection {
    #[arg(long, default_value = "perseid.toml")]
    pub config: PathBuf,
    #[arg(long = "target")]
    pub targets: Vec<String>,
}
#[derive(Clone, Debug, Args)]
pub struct Generation {
    #[command(flatten)]
    pub selection: Selection,
    #[arg(long)]
    pub no_format: bool,
    #[arg(long)]
    pub check: bool,
    /// Generate/check and create or update GitHub pull requests.
    #[arg(long, conflicts_with = "no_format")]
    pub pr: bool,
    /// Preview GitHub changes without pushing or creating PRs.
    #[arg(long, requires = "pr")]
    pub dry_run: bool,
}
#[derive(Clone, Debug, Subcommand)]
pub enum ProjectCommand {
    Sync {
        #[arg(long, default_value = "perseid.toml")]
        config: PathBuf,
    },
    Plan(Selection),
    Generate(Generation),
    Check(Selection),
    /// Compatibility alias for `generate --pr`.
    #[command(hide = true)]
    Propose {
        #[command(flatten)]
        selection: Selection,
        #[arg(long)]
        dry_run: bool,
    },
    Version {
        #[command(flatten)]
        selection: Selection,
        bump: String,
        #[arg(long)]
        notes: PathBuf,
    },
    Release {
        #[command(flatten)]
        selection: Selection,
        #[arg(long)]
        dry_run: bool,
    },
}
pub fn run(command: &ProjectCommand) -> Result<()> {
    let path = match command {
        ProjectCommand::Sync { config } => config,
        ProjectCommand::Plan(s) | ProjectCommand::Check(s) => &s.config,
        ProjectCommand::Generate(g) => &g.selection.config,
        ProjectCommand::Propose { selection, .. }
        | ProjectCommand::Version { selection, .. }
        | ProjectCommand::Release { selection, .. } => &selection.config,
    };
    let project = Project::load(path)?;
    let assets = Assets::load()?;
    let result = match command {
        ProjectCommand::Sync { .. } => source::sync(&project, &assets)?,
        ProjectCommand::Plan(s) => project.plan(&s.targets, &assets)?,
        ProjectCommand::Generate(g) => {
            if g.pr {
                github::generate_pr(&project, &g.selection.targets, &assets, g.dry_run)?
            } else {
                workspace::generate(
                    &project,
                    &g.selection.targets,
                    &assets,
                    false,
                    g.no_format,
                    g.check,
                )?
            }
        }
        ProjectCommand::Check(s) => {
            workspace::generate(&project, &s.targets, &assets, true, false, true)?
        }
        ProjectCommand::Propose { selection, dry_run } => {
            github::generate_pr(&project, &selection.targets, &assets, *dry_run)?
        }
        ProjectCommand::Version {
            selection,
            bump,
            notes,
        } => release::prepare_version(
            &project,
            &selection.targets,
            &assets,
            bump,
            &fs::read_to_string(notes)?,
        )?,
        ProjectCommand::Release { selection, dry_run } => {
            release::release(&project, &selection.targets, &assets, *dry_run)?
        }
    };
    println!("{}", serde_json::to_string_pretty(&result)?);
    if matches!(command, ProjectCommand::Check(_)) {
        ensure!(
            !workspace::has_changes(&result),
            "Generated SDKs are out of date"
        );
    }
    Ok(())
}

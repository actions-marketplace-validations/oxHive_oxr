use clap::{Parser, Subcommand};

use crate::release::{ForTarget, Level};

#[derive(Parser)]
#[command(
    name = "oxr",
    version,
    about = "Semantic-version bump and git-tag orchestrator for manifest-less repos"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Write a commented oxr.toml scaffold to the repo root.
    Init {
        /// Overwrite an existing oxr.toml.
        #[arg(long)]
        force: bool,
    },
    /// Bump the version and create a release tag (dry run unless --execute).
    Release {
        level: Level,
        /// Which component to bump when starting a fresh pre-release train,
        /// or the resolved patch/minor/major level itself.
        #[arg(long = "for")]
        for_target: Option<ForTarget>,
        /// Actually mutate the repo. Without this, oxr only prints its plan.
        #[arg(long)]
        execute: bool,
        /// Skip the confirmation prompt before releasing.
        #[arg(short = 'y', long = "yes")]
        yes: bool,
    },
    /// Move a floating major/minor tag to point at a stable release tag.
    Float {
        #[arg(long)]
        tag: String,
        /// Actually mutate the repo. Without this, oxr only prints its plan.
        #[arg(long)]
        execute: bool,
    },
    /// Print the resolved latest_stable and active_train, read-only.
    Current {
        #[arg(long)]
        json: bool,
    },
}

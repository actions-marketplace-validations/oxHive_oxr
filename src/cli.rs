use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::release::{ForTarget, Level};

#[derive(Parser)]
#[command(
    name = "oxr",
    version,
    about = "Semantic-version bump and git-tag orchestrator for manifest-less repos"
)]
pub struct Cli {
    /// Config file to use instead of oxr.toml/release.toml at the repo root,
    /// e.g. one per app in a monorepo. Must exist (except for `init`).
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,
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
        /// Which component a fresh alpha/beta/rc train bumps (default: patch).
        /// Only valid with alpha, beta, or rc.
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

mod cli;
mod config;
mod float;
mod git;
mod release;
mod replace;
mod template;
mod version;

use std::env;
use std::fmt::Display;
use std::io;
use std::path::Path;

use anstyle::AnsiColor;
use anyhow::{bail, Context, Result};
use clap::Parser;

use config::Config;
use release::{ForTarget, Level};
use version::Resolution;

fn main() -> Result<()> {
    if let Err(err) = run() {
        let s = AnsiColor::Red.on_default().bold();
        anstream::eprintln!("{s}error{s:#}: {err:#}");
        std::process::exit(1);
    }
    Ok(())
}

/// cargo-style progress line on stderr: a right-aligned bold verb, then the
/// message. anstream drops the color when stderr isn't a terminal or NO_COLOR
/// is set, so CI logs and pipes get plain text.
fn status(verb: &str, msg: impl Display) {
    let s = AnsiColor::Green.on_default().bold();
    anstream::eprintln!("{s}{verb:>12}{s:#} {msg}");
}

fn warn(msg: impl Display) {
    let s = AnsiColor::Yellow.on_default().bold();
    anstream::eprintln!("{s}warning{s:#}: {msg}");
}

fn run() -> Result<()> {
    let cli = cli::Cli::parse();
    let cwd = env::current_dir()?;
    let repo_root = git::repo_root(&cwd)?;

    // `init` only writes a scaffold file; it needs neither tag history nor
    // an existing config, so it's exempt from the shallow-checkout gate.
    match cli.command {
        cli::Command::Init { force } => return run_init(&repo_root, force),
        cli::Command::Current { .. }
        | cli::Command::Release { .. }
        | cli::Command::Float { .. } => {}
    }

    if git::is_shallow(&repo_root)? {
        bail!(
            "'{}' is a shallow git checkout, so tag history is incomplete and version \
             resolution would silently be wrong. If running in GitHub Actions, set \
             `fetch-depth: 0` on the actions/checkout step (the default fetch-depth: 1 \
             does not fetch tags).",
            repo_root.display()
        );
    }

    let config = config::load(&repo_root)?;

    match cli.command {
        cli::Command::Init { .. } => unreachable!("handled above"),
        cli::Command::Current { json } => run_current(&repo_root, &config, json),
        cli::Command::Release {
            level,
            for_target,
            execute,
            yes,
        } => run_release(&repo_root, &config, level, for_target, execute, yes),
        cli::Command::Float { tag, execute } => run_float(&repo_root, &config, &tag, execute),
    }
}

fn run_init(repo_root: &Path, force: bool) -> Result<()> {
    let target = repo_root.join("oxr.toml");
    let legacy = repo_root.join("release.toml");

    if target.exists() && !force {
        bail!(
            "'{}' already exists; pass --force to overwrite",
            target.display()
        );
    }

    std::fs::write(&target, config::SCAFFOLD)
        .with_context(|| format!("writing {}", target.display()))?;

    status("Created", target.display());
    if legacy.exists() {
        warn(format!(
            "'{}' also exists; oxr.toml now takes precedence over it",
            legacy.display()
        ));
    }
    Ok(())
}

fn resolve(repo_root: &Path, config: &Config) -> Result<Resolution> {
    let tags = git::list_tags(repo_root)?;
    version::resolve(&tags, &config.tag_pattern)
}

fn run_current(repo_root: &Path, config: &Config, json: bool) -> Result<()> {
    let resolution = resolve(repo_root, config)?;

    if json {
        let out = serde_json::json!({
            "latest_stable": resolution.latest_stable.as_ref().map(|v| v.to_string()),
            "active_train": resolution.active_train().map(|v| v.to_string()),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        match &resolution.latest_stable {
            Some(v) => println!("latest_stable: {v}"),
            None => println!("latest_stable: none (no tags yet)"),
        }
        match resolution.active_train() {
            Some(v) => println!("active_train:  {v}"),
            None => println!("active_train:  none"),
        }
    }
    Ok(())
}

fn confirm(prompt: &str) -> Result<bool> {
    eprint!("{prompt} [y/N] ");
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(matches!(input.trim().to_lowercase().as_str(), "y" | "yes"))
}

fn run_release(
    repo_root: &Path,
    config: &Config,
    level: Level,
    for_target: Option<ForTarget>,
    execute: bool,
    yes: bool,
) -> Result<()> {
    let resolution = resolve(repo_root, config)?;
    let next = release::next_version(&resolution, level, for_target)?;
    let tag_name = template::render(&config.tag_name, &next);

    if git::tag_exists(repo_root, &tag_name)? {
        bail!("tag '{tag_name}' already exists");
    }

    let name = repo_root
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    match &resolution.latest_stable {
        Some(from) => status("Upgrading", format!("{name} from {from} to {next}")),
        None => status("Upgrading", format!("{name} to {next}")),
    }

    if execute {
        if !yes && !confirm(&format!("release {tag_name}?"))? {
            warn("aborted");
            return Ok(());
        }
        if git::is_dirty(repo_root)? {
            bail!(
                "'{}' has uncommitted changes; commit or stash them before releasing so the \
                 release tag reflects a known state.",
                repo_root.display()
            );
        }
    }

    // Dry run and execute walk the same steps and print the same lines; only
    // execute mutates anything.
    let replacements = &config.pre_release_replacements;
    for r in replacements {
        status("Updating", &r.file);
        if execute {
            replace::apply(repo_root, r, &next)?;
        } else {
            replace::check(repo_root, r)?;
        }
    }

    let commit = !replacements.is_empty();
    let commit_message = template::render(&config.pre_release_commit_message, &next);
    if commit {
        status("Committing", &commit_message);
        if execute {
            let paths: Vec<String> = replacements.iter().map(|r| r.file.clone()).collect();
            git::stage_and_commit(repo_root, &paths, &commit_message, config.sign_commit)?;
        }
    }

    if config.tag {
        status("Tagging", &tag_name);
        if execute {
            git::create_tag(repo_root, &tag_name, &commit_message, config.sign_tag)?;
        }
    }

    if config.push && (commit || config.tag) {
        let mut refs = Vec::new();
        if commit {
            refs.push(git::current_branch(repo_root)?);
        }
        if config.tag {
            refs.push(tag_name.clone());
        }
        status("Pushing", format!("{} to origin", refs.join(", ")));
        if execute {
            if commit {
                git::push_current_branch(repo_root)?;
            }
            if config.tag {
                git::push_tag(repo_root, &tag_name, false)?;
            }
        }
    }

    if execute {
        status("Released", &tag_name);
    } else {
        warn("aborting release due to dry run; re-run with --execute");
    }
    Ok(())
}

fn run_float(repo_root: &Path, config: &Config, tag: &str, execute: bool) -> Result<()> {
    if !git::tag_exists(repo_root, tag)? {
        bail!("tag '{tag}' does not exist locally; fetch it first");
    }

    let plan = float::plan(tag, &config.float_tags)?;

    if plan.tags.is_empty() {
        warn(
            "no floating tags enabled in [float-tags] (major and minor both false); nothing to do",
        );
        return Ok(());
    }

    let target_sha = git::commit_of(repo_root, tag)?;
    let message = format!("float {tag}");
    for floating in &plan.tags {
        status(
            "Floating",
            format!("{floating} to {tag} ({})", plan.version),
        );
        if execute {
            git::force_move_tag(repo_root, floating, &target_sha, &message, config.sign_tag)?;
        }
    }

    if config.push {
        status("Pushing", format!("{} to origin", plan.tags.join(", ")));
        if execute {
            for floating in &plan.tags {
                git::push_tag(repo_root, floating, true)?;
            }
        }
    }

    if !execute {
        warn("aborting float due to dry run; re-run with --execute");
    }
    Ok(())
}

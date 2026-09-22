mod cli;
mod config;
mod float;
mod git;
mod release;
mod replace;
mod template;
mod version;

use std::env;
use std::io::{self, Write};
use std::path::Path;

use anyhow::{bail, Context, Result};
use clap::Parser;

use config::Config;
use release::{ForTarget, Level};
use version::Resolution;

fn main() -> Result<()> {
    if let Err(err) = run() {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
    Ok(())
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

    println!("wrote {}", target.display());
    if legacy.exists() {
        println!(
            "note: '{}' also exists; oxr.toml now takes precedence over it",
            legacy.display()
        );
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
    print!("{prompt} [y/N] ");
    io::stdout().flush()?;
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

    let from = resolution
        .latest_stable
        .as_ref()
        .map(|v| v.to_string())
        .unwrap_or_else(|| "none".to_string());
    println!("{from} -> {next}  (tag: {tag_name})");

    if !execute {
        println!("(dry run; pass --execute to apply)");
        for r in &config.pre_release_replacements {
            replace::check(repo_root, r)?;
            println!(
                "would update {} ({} match(es) of the search pattern, as expected)",
                r.file, r.exactly
            );
        }
        if config.tag {
            println!("would create tag {tag_name}");
        }
        if config.push {
            println!("would push commit and tag to origin");
        }
        return Ok(());
    }

    if !yes && !confirm(&format!("release {tag_name}?"))? {
        println!("aborted");
        return Ok(());
    }

    if git::is_dirty(repo_root)? {
        bail!(
            "'{}' has uncommitted changes; commit or stash them before releasing so the \
             release tag reflects a known state.",
            repo_root.display()
        );
    }

    let mut changed_paths = Vec::new();
    for r in &config.pre_release_replacements {
        replace::apply(repo_root, r, &next)?;
        changed_paths.push(r.file.clone());
        println!("updated {}", r.file);
    }

    let commit_message = template::render(&config.pre_release_commit_message, &next);

    if !changed_paths.is_empty() {
        git::stage_and_commit(
            repo_root,
            &changed_paths,
            &commit_message,
            config.sign_commit,
        )?;
        println!("committed \"{commit_message}\"");
    }

    if config.tag {
        git::create_tag(repo_root, &tag_name, &commit_message, config.sign_tag)?;
        println!("created tag {tag_name}");
    }

    if config.push {
        if !changed_paths.is_empty() {
            git::push_current_branch(repo_root)?;
            println!("pushed commit to origin");
        }
        if config.tag {
            git::push_tag(repo_root, &tag_name, false)?;
            println!("pushed tag {tag_name} to origin");
        }
    }

    println!("released {tag_name}");
    Ok(())
}

fn run_float(repo_root: &Path, config: &Config, tag: &str, execute: bool) -> Result<()> {
    if !git::tag_exists(repo_root, tag)? {
        bail!("tag '{tag}' does not exist locally; fetch it first");
    }

    let plan = float::plan(tag, &config.float_tags)?;

    if plan.tags.is_empty() {
        println!(
            "no floating tags enabled in [float-tags] (major and minor both false); nothing to do"
        );
        return Ok(());
    }

    for floating in &plan.tags {
        println!("{floating} -> {tag} ({})", plan.version);
    }

    if !execute {
        println!("(dry run; pass --execute to apply)");
        return Ok(());
    }

    let target_sha = git::commit_of(repo_root, tag)?;
    let message = format!("float {tag}");
    for floating in &plan.tags {
        git::force_move_tag(repo_root, floating, &target_sha, &message, config.sign_tag)?;
        if config.push {
            git::push_tag(repo_root, floating, true)?;
        }
    }

    println!("floated: {}", plan.tags.join(", "));
    Ok(())
}

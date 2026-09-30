mod cli;
mod config;
mod float;
mod git;
mod release;
mod replace;
mod template;
mod version;

use std::env;
use std::io::{self, IsTerminal, Write};
use std::path::Path;

use anyhow::{bail, Context, Result};
use clap::Parser;
use semver::Version;

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

/// Errors unless `tag_name` would be picked up again by version resolution:
/// it must match `tag-pattern` and carry exactly `next`. Otherwise every
/// new tag is invisible and each release recomputes the same version.
fn check_tag_name_round_trips(config: &Config, tag_name: &str, next: &Version) -> Result<()> {
    let pattern = version::compile_pattern(&config.tag_pattern)?;
    if !pattern.is_match(tag_name) || version::extract_version(tag_name).as_ref() != Some(next) {
        bail!(
            "tag-name renders '{tag_name}', which tag-pattern '{}' would not resolve back to \
             {next}; the new tag would be invisible to the next release. Make tag-name and \
             tag-pattern agree.",
            config.tag_pattern
        );
    }
    Ok(())
}

/// Errors if origin has release tags this clone hasn't fetched: the next
/// version would be computed from stale local state.
fn check_remote_tags_fetched(repo_root: &Path, config: &Config) -> Result<()> {
    let pattern = version::compile_pattern(&config.tag_pattern)?;
    let local = git::list_tags(repo_root)?;
    let missing: Vec<String> = git::remote_tags(repo_root, "origin")
        .context("checking origin's tags before releasing")?
        .into_iter()
        .filter(|t| pattern.is_match(t) && !local.contains(t))
        .collect();
    if !missing.is_empty() {
        bail!(
            "origin has release tags this clone hasn't fetched ({}); run `git fetch --tags` \
             so the next version isn't computed from stale tags",
            missing.join(", ")
        );
    }
    Ok(())
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

    if config.tag {
        check_tag_name_round_trips(config, &tag_name, &next)?;
    }
    if git::tag_exists(repo_root, &tag_name)? {
        bail!("tag '{tag_name}' already exists");
    }

    // Validates every replacement entry before anything is written.
    let changes = replace::plan(repo_root, &config.pre_release_replacements, &next)?;
    let will_commit = changes.iter().any(|c| c.changed());

    let from = resolution
        .latest_stable
        .as_ref()
        .map(|v| v.to_string())
        .unwrap_or_else(|| "none".to_string());
    println!("{from} -> {next}  (tag: {tag_name})");

    if !execute {
        println!("(dry run; pass --execute to apply)");
        for c in &changes {
            if c.changed() {
                println!("would update {}", c.file);
            } else {
                println!("{} is already up to date", c.file);
            }
        }
        if will_commit {
            println!(
                "would commit \"{}\"",
                template::render(&config.pre_release_commit_message, &next)
            );
        }
        if config.tag {
            println!("would create tag {tag_name}");
        }
        if config.push && (will_commit || config.tag) {
            println!("would push to origin");
        }
        return Ok(());
    }

    // Every precondition is checked before prompting, so answering "y"
    // never leads straight into an avoidable error.
    if git::is_dirty(repo_root)? {
        bail!(
            "'{}' has uncommitted changes; commit or stash them before releasing so the \
             release tag reflects a known state.",
            repo_root.display()
        );
    }
    let branch = match git::current_branch(repo_root)? {
        Some(b) => Some(b),
        None if will_commit && config.push => bail!(
            "HEAD is detached, so the release commit would not be on any branch; check out \
             the branch to release from first"
        ),
        None => None,
    };
    if config.push {
        check_remote_tags_fetched(repo_root, config)?;
    }

    if !yes {
        if !io::stdin().is_terminal() {
            bail!("refusing to release without confirmation: stdin is not a terminal; pass --yes to release non-interactively");
        }
        if !confirm(&format!("release {tag_name}?"))? {
            bail!("aborted; nothing was changed");
        }
    }

    let orig_head = git::head_sha(repo_root)?;
    let mut tag_created = false;
    let result = execute_release(
        repo_root,
        config,
        &changes,
        &next,
        &tag_name,
        branch.as_deref(),
        &mut tag_created,
    );

    if let Err(err) = result {
        let mut rollback = Vec::new();
        if tag_created {
            if let Err(e) = git::delete_tag(repo_root, &tag_name) {
                rollback.push(format!("deleting tag {tag_name}: {e:#}"));
            }
        }
        if let Err(e) = git::reset_hard(repo_root, &orig_head) {
            rollback.push(format!("resetting to {orig_head}: {e:#}"));
        }
        if rollback.is_empty() {
            return Err(err.context(
                "release failed; rolled back the local release commit, tag, and file \
                 changes (nothing was pushed)",
            ));
        }
        return Err(err.context(format!(
            "release failed, and rolling it back also failed ({}); inspect the repo manually",
            rollback.join("; ")
        )));
    }

    println!("released {tag_name}");
    Ok(())
}

fn execute_release(
    repo_root: &Path,
    config: &Config,
    changes: &[replace::FileChange],
    next: &Version,
    tag_name: &str,
    branch: Option<&str>,
    tag_created: &mut bool,
) -> Result<()> {
    replace::write(changes)?;
    let changed: Vec<String> = changes
        .iter()
        .filter(|c| c.changed())
        .map(|c| c.file.clone())
        .collect();
    for file in &changed {
        println!("updated {file}");
    }
    for c in changes.iter().filter(|c| !c.changed()) {
        println!("{} already up to date", c.file);
    }

    let commit_message = template::render(&config.pre_release_commit_message, next);
    if !changed.is_empty() {
        git::stage_and_commit(repo_root, &changed, &commit_message, config.sign_commit)?;
        println!("committed \"{commit_message}\"");
    }

    if config.tag {
        git::create_tag(repo_root, tag_name, &commit_message, config.sign_tag)?;
        *tag_created = true;
        println!("created tag {tag_name}");
    }

    if config.push {
        let mut refs = Vec::new();
        if !changed.is_empty() {
            let branch = branch.expect("detached HEAD rejected before committing");
            refs.push(format!("refs/heads/{branch}"));
        }
        if config.tag {
            refs.push(format!("refs/tags/{tag_name}"));
        }
        if !refs.is_empty() {
            git::push_refs(repo_root, &refs, false)?;
            println!("pushed {} to origin", refs.join(", "));
        }
    }
    Ok(())
}

fn run_float(repo_root: &Path, config: &Config, tag: &str, execute: bool) -> Result<()> {
    if !git::tag_exists(repo_root, tag)? {
        bail!("tag '{tag}' does not exist locally; fetch it first");
    }
    if !version::compile_pattern(&config.tag_pattern)?.is_match(tag) {
        bail!(
            "tag '{tag}' does not match tag-pattern '{}'; only release tags can be floated",
            config.tag_pattern
        );
    }

    let known = version::matching_versions(&git::list_tags(repo_root)?, &config.tag_pattern)?;
    let plan = float::plan(tag, &config.float_tags, &known)?;

    for (floating, newer) in &plan.skipped {
        println!("{floating} stays put: {newer} is a newer stable release in its line");
    }

    if plan.tags.is_empty() {
        if plan.skipped.is_empty() {
            println!(
                "no floating tags enabled in [float-tags] (major and minor both false); nothing to do"
            );
        } else {
            println!("nothing to do");
        }
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
    }
    if config.push {
        let refs: Vec<String> = plan.tags.iter().map(|t| format!("refs/tags/{t}")).collect();
        git::push_refs(repo_root, &refs, true)?;
    }

    println!("floated: {}", plan.tags.join(", "));
    Ok(())
}

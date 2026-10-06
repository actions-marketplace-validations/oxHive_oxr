//! Integration tests that spawn the real `oxr` binary against scratch git
//! repos, covering the CLI/orchestration layer (`main.rs`) that the unit
//! tests in `src/` can't reach on their own.

use std::path::Path;
use std::process::{Command, Output};

fn oxr() -> Command {
    Command::new(env!("CARGO_BIN_EXE_oxr"))
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(status.success(), "`git {}` failed", args.join(" "));
}

fn init_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q"]);
    git(dir.path(), &["config", "user.email", "t@example.com"]);
    git(dir.path(), &["config", "user.name", "t"]);
    git(dir.path(), &["commit", "-q", "--allow-empty", "-m", "init"]);
    dir
}

fn run(dir: &Path, args: &[&str]) -> Output {
    oxr().args(args).current_dir(dir).output().unwrap()
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).to_string()
}

fn git_out(dir: &Path, args: &[&str]) -> String {
    let o = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(o.status.success(), "`git {}` failed", args.join(" "));
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

fn tags(dir: &Path) -> Vec<String> {
    git_out(dir, &["tag", "-l"])
        .lines()
        .map(|s| s.to_string())
        .collect()
}

/// Adds a fresh bare repo as `origin`. Deliberately does NOT set
/// `push.autoSetupRemote` or an upstream: oxr must push explicitly.
fn add_origin(dir: &Path) -> tempfile::TempDir {
    let bare = tempfile::tempdir().unwrap();
    git(bare.path(), &["init", "-q", "--bare"]);
    git(
        dir,
        &["remote", "add", "origin", bare.path().to_str().unwrap()],
    );
    bare
}

fn commit_all(dir: &Path, message: &str) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", message]);
}

const PLUGIN_REPLACEMENT: &str = r#"
[[pre-release-replacements]]
file = "plugin.json"
search = "\"version\": \"[^\"]+\""
replace = "\"version\": \"{{version}}\""
exactly = 1
"#;

#[test]
fn init_writes_oxr_toml_and_respects_force() {
    let dir = init_repo();

    let o = run(dir.path(), &["init"]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(err(&o).contains("Created"), "{}", err(&o));
    assert!(dir.path().join("oxr.toml").exists());

    let o = run(dir.path(), &["init"]);
    assert!(!o.status.success());
    assert!(err(&o).contains("already exists"));

    let o = run(dir.path(), &["init", "--force"]);
    assert!(o.status.success(), "{}", err(&o));
}

#[test]
fn init_notes_a_coexisting_release_toml() {
    let dir = init_repo();
    std::fs::write(dir.path().join("release.toml"), "push = false\n").unwrap();

    let o = run(dir.path(), &["init"]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(err(&o).contains("release.toml"));
    assert!(err(&o).contains("takes precedence"));
}

#[test]
fn current_reports_none_on_an_empty_repo_text_and_json() {
    let dir = init_repo();

    let o = run(dir.path(), &["current"]);
    assert!(o.status.success());
    assert!(out(&o).contains("latest_stable: none"));
    assert!(out(&o).contains("active_train:  none"));

    let o = run(dir.path(), &["current", "--json"]);
    assert!(o.status.success());
    let v: serde_json::Value = serde_json::from_str(&out(&o)).unwrap();
    assert!(v["latest_stable"].is_null());
    assert!(v["active_train"].is_null());
}

#[test]
fn current_text_shows_values_when_tags_exist() {
    let dir = init_repo();
    git(dir.path(), &["tag", "v1.0.0"]);
    git(dir.path(), &["commit", "-q", "--allow-empty", "-m", "c2"]);
    git(dir.path(), &["tag", "v1.1.0-rc.1"]);

    let o = run(dir.path(), &["current"]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(out(&o).contains("latest_stable: 1.0.0"), "{}", out(&o));
    assert!(out(&o).contains("active_train:  1.1.0-rc.1"), "{}", out(&o));
}

#[test]
fn release_dry_run_previews_pending_replacements() {
    let dir = init_repo();
    std::fs::write(
        dir.path().join("oxr.toml"),
        r#"
[[pre-release-replacements]]
file = "plugin.json"
search = "\"version\": \"[^\"]+\""
replace = "\"version\": \"{{version}}\""
exactly = 1
"#,
    )
    .unwrap();
    std::fs::write(dir.path().join("plugin.json"), r#"{"version": "0.0.0"}"#).unwrap();

    let o = run(dir.path(), &["release", "patch"]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(err(&o).contains("Updating plugin.json"), "{}", err(&o));
}

#[test]
fn release_dry_run_catches_a_bad_search_pattern_before_execute() {
    let dir = init_repo();
    std::fs::write(
        dir.path().join("oxr.toml"),
        r#"
[[pre-release-replacements]]
file = "plugin.json"
search = "\"varsion\": \"[^\"]+\""
replace = "\"varsion\": \"{{version}}\""
exactly = 1
"#,
    )
    .unwrap();
    std::fs::write(dir.path().join("plugin.json"), r#"{"version": "0.0.0"}"#).unwrap();

    let o = run(dir.path(), &["release", "patch"]);
    assert!(!o.status.success());
    assert!(err(&o).contains("found 0"), "{}", err(&o));
}

#[test]
fn shallow_checkout_blocks_current_release_and_float_but_not_init() {
    let src = init_repo();
    git(src.path(), &["tag", "v1.0.0"]);
    git(src.path(), &["commit", "-q", "--allow-empty", "-m", "c2"]);

    let dst = tempfile::tempdir().unwrap();
    let src_url = format!("file://{}", src.path().display());
    git(dst.path(), &["clone", "-q", "--depth", "1", &src_url, "."]);

    for args in [
        &["current"][..],
        &["release", "patch"][..],
        &["float", "--tag", "v1.0.0"][..],
    ] {
        let o = run(dst.path(), args);
        assert!(!o.status.success());
        assert!(err(&o).contains("shallow git checkout"), "{}", err(&o));
    }

    let o = run(dst.path(), &["init"]);
    assert!(o.status.success(), "{}", err(&o));
}

#[test]
fn release_dry_run_prints_plan_and_mutates_nothing() {
    let dir = init_repo();

    let o = run(dir.path(), &["release", "patch"]);
    assert!(o.status.success(), "{}", err(&o));
    let text = err(&o);
    assert!(text.contains(" to 0.1.0"), "{text}");
    assert!(text.contains("Tagging v0.1.0"), "{text}");
    assert!(text.contains("re-run with --execute"), "{text}");
    assert!(out(&o).is_empty(), "status output belongs on stderr");

    assert!(run(dir.path(), &["current", "--json"])
        .stdout
        .starts_with(b"{"));
    let tags = String::from_utf8(
        Command::new("git")
            .args(["tag", "-l"])
            .current_dir(dir.path())
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert!(tags.trim().is_empty(), "dry run must not create tags");
}

#[test]
fn release_execute_full_lifecycle_with_replacements_and_push() {
    let bare = tempfile::tempdir().unwrap();
    git(bare.path(), &["init", "-q", "--bare"]);

    let dir = init_repo();
    git(dir.path(), &["config", "push.autoSetupRemote", "true"]);
    git(
        dir.path(),
        &["remote", "add", "origin", bare.path().to_str().unwrap()],
    );

    std::fs::write(
        dir.path().join("oxr.toml"),
        r#"
[[pre-release-replacements]]
file = "plugin.json"
search = "\"version\": \"[^\"]+\""
replace = "\"version\": \"{{version}}\""
exactly = 1

[float-tags]
major = true
minor = true
"#,
    )
    .unwrap();
    std::fs::write(dir.path().join("plugin.json"), r#"{"version": "0.0.0"}"#).unwrap();
    git(dir.path(), &["add", "oxr.toml", "plugin.json"]);
    git(dir.path(), &["commit", "-q", "-m", "add config"]);

    // Bootstrap: first release always targets minor regardless of level.
    let o = run(dir.path(), &["release", "patch", "--execute", "--yes"]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(err(&o).contains(" to 0.1.0"), "{}", err(&o));
    let plugin = std::fs::read_to_string(dir.path().join("plugin.json")).unwrap();
    assert!(plugin.contains("0.1.0"), "{plugin}");

    // A real bump, with the replacement, commit, tag, and push all firing.
    let o = run(dir.path(), &["release", "minor", "--execute", "--yes"]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(err(&o).contains("from 0.1.0 to 0.2.0"), "{}", err(&o));
    let plugin = std::fs::read_to_string(dir.path().join("plugin.json")).unwrap();
    assert!(plugin.contains("0.2.0"), "{plugin}");
    let remote_tags = String::from_utf8(
        Command::new("git")
            .args(["tag", "-l"])
            .current_dir(bare.path())
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert!(remote_tags.contains("v0.2.0"), "{remote_tags}");

    // Start, advance, and finalize a pre-release train.
    let o = run(dir.path(), &["release", "rc", "--execute", "--yes"]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(err(&o).contains("from 0.2.0 to 0.2.1-rc.1"), "{}", err(&o));

    let o = run(dir.path(), &["release", "beta"]);
    assert!(!o.status.success());
    assert!(err(&o).contains("pre-release maturity only moves forward"));

    let o = run(dir.path(), &["release", "rc", "--execute", "--yes"]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(err(&o).contains("0.2.1-rc.2"));

    let o = run(dir.path(), &["release", "stable", "--execute", "--yes"]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(err(&o).contains("Released v0.2.1"), "{}", err(&o));

    // Float major and minor, then confirm a major bump never touches v0.
    let o = run(dir.path(), &["float", "--tag", "v0.2.1", "--execute"]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(err(&o).contains("Floating v0 to v0.2.1"), "{}", err(&o));
    assert!(err(&o).contains("Floating v0.2 to v0.2.1"), "{}", err(&o));

    let v0_before = String::from_utf8(
        Command::new("git")
            .args(["rev-list", "-n", "1", "v0"])
            .current_dir(dir.path())
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();

    let o = run(dir.path(), &["release", "major", "--execute", "--yes"]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(err(&o).contains("from 0.2.1 to 1.0.0"), "{}", err(&o));

    let o = run(dir.path(), &["float", "--tag", "v1.0.0", "--execute"]);
    assert!(o.status.success(), "{}", err(&o));

    let v0_after = String::from_utf8(
        Command::new("git")
            .args(["rev-list", "-n", "1", "v0"])
            .current_dir(dir.path())
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(v0_before, v0_after, "major bump must not touch v0");

    let v1 = String::from_utf8(
        Command::new("git")
            .args(["rev-list", "-n", "1", "v1"])
            .current_dir(dir.path())
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_ne!(v0_after, v1);
}

#[test]
fn release_execute_refuses_a_dirty_working_tree_but_dry_run_still_previews() {
    let dir = init_repo();
    std::fs::write(dir.path().join("untracked"), "x").unwrap();

    // Dry run only prints a plan, so an unrelated dirty file (e.g. a config
    // edit not yet committed) shouldn't block it.
    let o = run(dir.path(), &["release", "patch"]);
    assert!(o.status.success(), "{}", err(&o));

    let o = run(dir.path(), &["release", "patch", "--execute", "--yes"]);
    assert!(!o.status.success());
    assert!(err(&o).contains("uncommitted changes"), "{}", err(&o));
}

#[test]
fn release_execute_without_yes_fails_when_stdin_is_not_a_terminal() {
    // Regression: with no terminal the prompt read EOF, printed "aborted",
    // and exited 0, so a CI job missing --yes went green having released
    // nothing. `run` uses Output::output(), so stdin is not a terminal.
    let dir = init_repo();
    std::fs::write(dir.path().join("oxr.toml"), "push = false\n").unwrap();
    commit_all(dir.path(), "cfg");

    let o = run(dir.path(), &["release", "patch", "--execute"]);
    assert!(!o.status.success(), "{}", out(&o));
    assert!(err(&o).contains("pass --yes"), "{}", err(&o));
    assert!(tags(dir.path()).is_empty());
}

#[test]
fn release_preconditions_are_checked_before_the_prompt() {
    // A dirty tree or a broken replacement must surface before asking for
    // confirmation, not after the user has already said yes.
    let dir = init_repo();
    std::fs::write(dir.path().join("untracked"), "x").unwrap();
    let o = run(dir.path(), &["release", "patch", "--execute"]);
    assert!(err(&o).contains("uncommitted changes"), "{}", err(&o));

    let dir = init_repo();
    std::fs::write(dir.path().join("oxr.toml"), PLUGIN_REPLACEMENT).unwrap();
    std::fs::write(dir.path().join("plugin.json"), "{}").unwrap();
    commit_all(dir.path(), "cfg");
    let o = run(dir.path(), &["release", "patch", "--execute"]);
    assert!(err(&o).contains("found 0"), "{}", err(&o));
}

#[test]
fn release_execute_prints_progress_for_each_step() {
    let bare = tempfile::tempdir().unwrap();
    git(bare.path(), &["init", "-q", "--bare"]);

    let dir = init_repo();
    git(dir.path(), &["config", "push.autoSetupRemote", "true"]);
    git(
        dir.path(),
        &["remote", "add", "origin", bare.path().to_str().unwrap()],
    );

    std::fs::write(
        dir.path().join("oxr.toml"),
        r#"
[[pre-release-replacements]]
file = "plugin.json"
search = "\"version\": \"[^\"]+\""
replace = "\"version\": \"{{version}}\""
exactly = 1
"#,
    )
    .unwrap();
    std::fs::write(dir.path().join("plugin.json"), r#"{"version": "0.0.0"}"#).unwrap();
    git(dir.path(), &["add", "oxr.toml", "plugin.json"]);
    git(dir.path(), &["commit", "-q", "-m", "add config"]);

    let o = run(dir.path(), &["release", "patch", "--execute", "--yes"]);
    assert!(o.status.success(), "{}", err(&o));
    let text = err(&o);
    let branch = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(dir.path())
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert!(text.contains("    Updating plugin.json"), "{text}");
    assert!(
        text.contains("  Committing chore: release v0.1.0"),
        "{text}"
    );
    assert!(text.contains("     Tagging v0.1.0"), "{text}");
    assert!(
        text.contains(&format!("     Pushing {}, v0.1.0 to origin", branch.trim())),
        "{text}"
    );
    assert!(text.contains("    Released v0.1.0"), "{text}");
    // Piped stderr is not a terminal, so no ANSI escapes leak into logs.
    assert!(!text.contains('\x1b'), "{text}");
    assert!(out(&o).is_empty(), "{}", out(&o));
}

#[test]
fn release_refuses_a_tag_name_that_tag_pattern_cannot_see() {
    // Regression: with tag-name and tag-pattern out of sync, new tags were
    // invisible to resolution; the first release worked and the second
    // recomputed the same version and failed with "already exists".
    let dir = init_repo();
    std::fs::write(
        dir.path().join("oxr.toml"),
        "push = false\ntag-name = \"release-{{version}}\"\n",
    )
    .unwrap();
    commit_all(dir.path(), "cfg");

    let o = run(dir.path(), &["release", "minor"]);
    assert!(!o.status.success());
    assert!(err(&o).contains("would not resolve back"), "{}", err(&o));
    assert!(tags(dir.path()).is_empty());
}

#[test]
fn unknown_config_keys_are_rejected() {
    let dir = init_repo();
    std::fs::write(dir.path().join("oxr.toml"), "pushh = false\n").unwrap();
    let o = run(dir.path(), &["release", "patch"]);
    assert!(!o.status.success());
    assert!(err(&o).contains("unknown field"), "{}", err(&o));
}

#[test]
fn release_pushes_without_an_upstream_configured() {
    // Regression: the branch push was a bare `git push`, which fails with
    // no upstream after the commit and tag already existed locally.
    let dir = init_repo();
    let bare = add_origin(dir.path());
    std::fs::write(dir.path().join("oxr.toml"), PLUGIN_REPLACEMENT).unwrap();
    std::fs::write(dir.path().join("plugin.json"), r#"{"version": "0.0.0"}"#).unwrap();
    commit_all(dir.path(), "cfg");

    let o = run(dir.path(), &["release", "minor", "--execute", "--yes"]);
    assert!(o.status.success(), "{}", err(&o));
    assert_eq!(tags(bare.path()), vec!["v0.1.0".to_string()]);
    assert_eq!(
        git_out(bare.path(), &["log", "-1", "--pretty=%s"]),
        "chore: release v0.1.0"
    );
}

#[test]
fn failed_push_rolls_back_so_a_retry_releases_the_same_version() {
    // Regression: a rejected push left the commit and tag local, and the
    // retry saw that tag and bumped again (v0.1.0, then v0.2.0, ...).
    let dir = init_repo();
    let bare = add_origin(dir.path());
    std::fs::write(dir.path().join("oxr.toml"), PLUGIN_REPLACEMENT).unwrap();
    std::fs::write(dir.path().join("plugin.json"), r#"{"version": "0.0.0"}"#).unwrap();
    commit_all(dir.path(), "cfg");
    let branch = git_out(dir.path(), &["rev-parse", "--abbrev-ref", "HEAD"]);
    let head_before = git_out(dir.path(), &["rev-parse", "HEAD"]);

    // Make origin's branch diverge so the push is rejected as non-fast-forward.
    let other = tempfile::tempdir().unwrap();
    git(
        other.path(),
        &["clone", "-q", bare.path().to_str().unwrap(), "."],
    );
    git(other.path(), &["config", "user.email", "t@example.com"]);
    git(other.path(), &["config", "user.name", "t"]);
    git(other.path(), &["checkout", "-q", "-b", &branch]);
    git(
        other.path(),
        &["commit", "-q", "--allow-empty", "-m", "elsewhere"],
    );
    git(other.path(), &["push", "-q", "origin", &branch]);

    for _ in 0..2 {
        let o = run(dir.path(), &["release", "minor", "--execute", "--yes"]);
        assert!(!o.status.success());
        assert!(err(&o).contains(" to 0.1.0"), "{}", err(&o));
        assert!(!err(&o).contains("0.2.0"), "{}", err(&o));
        assert!(err(&o).contains("rolled back"), "{}", err(&o));

        assert!(tags(dir.path()).is_empty());
        assert_eq!(git_out(dir.path(), &["rev-parse", "HEAD"]), head_before);
        assert_eq!(git_out(dir.path(), &["status", "--porcelain"]), "");
        assert!(
            tags(bare.path()).is_empty(),
            "atomic push must not land the tag"
        );
    }
}

#[test]
fn release_refuses_when_origin_has_unfetched_tags() {
    let dir = init_repo();
    let bare = add_origin(dir.path());
    git(dir.path(), &["tag", "v0.5.0"]);
    git(dir.path(), &["push", "-q", "origin", "refs/tags/v0.5.0"]);
    git(dir.path(), &["tag", "-d", "v0.5.0"]);
    assert_eq!(tags(bare.path()), vec!["v0.5.0".to_string()]);

    let o = run(dir.path(), &["release", "patch", "--execute", "--yes"]);
    assert!(!o.status.success());
    assert!(err(&o).contains("git fetch --tags"), "{}", err(&o));
    assert!(err(&o).contains("v0.5.0"), "{}", err(&o));
    assert!(tags(dir.path()).is_empty());
}

#[test]
fn release_with_a_no_op_replacement_tags_without_committing() {
    // Regression: a replacement that rendered identical text (a
    // `{{major}}`-only reference on a patch release) was still committed,
    // and `git commit` failed with nothing to commit.
    let dir = init_repo();
    std::fs::write(
        dir.path().join("oxr.toml"),
        r#"push = false
[[pre-release-replacements]]
file = "README.md"
search = 'oxhive/oxr@v\d+'
replace = 'oxhive/oxr@v{{major}}'
exactly = 1
"#,
    )
    .unwrap();
    std::fs::write(dir.path().join("README.md"), "uses: oxhive/oxr@v1\n").unwrap();
    commit_all(dir.path(), "cfg");
    git(dir.path(), &["tag", "v1.0.0"]);
    let head_before = git_out(dir.path(), &["rev-parse", "HEAD"]);

    let o = run(dir.path(), &["release", "patch"]);
    assert!(
        err(&o).contains("Skipping README.md (already up to date)"),
        "{}",
        err(&o)
    );

    let o = run(dir.path(), &["release", "patch", "--execute", "--yes"]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(!err(&o).contains("Committing"), "{}", err(&o));
    assert_eq!(git_out(dir.path(), &["rev-parse", "HEAD"]), head_before);
    assert!(tags(dir.path()).contains(&"v1.0.1".to_string()));
}

#[test]
fn release_with_a_bad_second_replacement_leaves_the_tree_clean() {
    // Regression: the first file was rewritten before the second entry
    // was validated, leaving a dirty tree behind.
    let dir = init_repo();
    std::fs::write(
        dir.path().join("oxr.toml"),
        format!(
            "push = false\n{}{}",
            PLUGIN_REPLACEMENT,
            PLUGIN_REPLACEMENT.replace("plugin.json", "other.json")
        ),
    )
    .unwrap();
    std::fs::write(dir.path().join("plugin.json"), r#"{"version": "0.0.0"}"#).unwrap();
    std::fs::write(dir.path().join("other.json"), "{}").unwrap();
    commit_all(dir.path(), "cfg");

    let o = run(dir.path(), &["release", "patch", "--execute", "--yes"]);
    assert!(!o.status.success());
    assert!(err(&o).contains("other.json"), "{}", err(&o));
    assert_eq!(git_out(dir.path(), &["status", "--porcelain"]), "");
}

#[test]
fn release_refuses_to_commit_on_a_detached_head() {
    let dir = init_repo();
    let _bare = add_origin(dir.path());
    std::fs::write(dir.path().join("oxr.toml"), PLUGIN_REPLACEMENT).unwrap();
    std::fs::write(dir.path().join("plugin.json"), r#"{"version": "0.0.0"}"#).unwrap();
    commit_all(dir.path(), "cfg");
    git(dir.path(), &["checkout", "-q", "--detach"]);
    let head_before = git_out(dir.path(), &["rev-parse", "HEAD"]);

    let o = run(dir.path(), &["release", "patch", "--execute", "--yes"]);
    assert!(!o.status.success());
    assert!(err(&o).contains("HEAD is detached"), "{}", err(&o));
    assert_eq!(git_out(dir.path(), &["rev-parse", "HEAD"]), head_before);
    assert_eq!(git_out(dir.path(), &["status", "--porcelain"]), "");
    assert!(tags(dir.path()).is_empty());
}

#[test]
fn for_flag_rejected_on_direct_levels_via_cli() {
    let dir = init_repo();
    let o = run(dir.path(), &["release", "patch", "--for", "major"]);
    assert!(!o.status.success());
    assert!(err(&o).contains("--for is only valid"));
}

#[test]
fn float_hard_refuses_prerelease_and_missing_tag() {
    let dir = init_repo();
    git(dir.path(), &["tag", "v1.0.0-rc.1"]);

    let o = run(dir.path(), &["float", "--tag", "v1.0.0-rc.1"]);
    assert!(!o.status.success());
    assert!(err(&o).contains("refusing to float a pre-release tag"));

    let o = run(dir.path(), &["float", "--tag", "v9.9.9"]);
    assert!(!o.status.success());
    assert!(err(&o).contains("does not exist locally"));
}

#[test]
fn float_with_no_floating_tags_enabled_is_a_no_op() {
    let dir = init_repo();
    git(dir.path(), &["tag", "v1.0.0"]);
    std::fs::write(
        dir.path().join("oxr.toml"),
        "[float-tags]\nmajor = false\nminor = false\n",
    )
    .unwrap();

    let o = run(dir.path(), &["float", "--tag", "v1.0.0", "--execute"]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(err(&o).contains("nothing to do"));
}

#[test]
fn float_dry_run_does_not_move_the_tag() {
    let dir = init_repo();
    git(dir.path(), &["tag", "v1.0.0"]);

    let o = run(dir.path(), &["float", "--tag", "v1.0.0"]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(err(&o).contains("re-run with --execute"));

    let exists = Command::new("git")
        .args(["rev-parse", "--verify", "-q", "refs/tags/v1"])
        .current_dir(dir.path())
        .status()
        .unwrap()
        .success();
    assert!(!exists, "dry run must not create the floating tag");
}

#[test]
fn float_skips_a_major_tag_that_a_newer_minor_already_owns() {
    // Regression: floating a hotfix for an older minor (v1.2.1 after
    // v1.3.0) moved `v1` backwards.
    let dir = init_repo();
    std::fs::write(
        dir.path().join("oxr.toml"),
        "push = false\n[float-tags]\nminor = true\n",
    )
    .unwrap();
    commit_all(dir.path(), "cfg");
    git(dir.path(), &["tag", "v1.3.0"]);
    let o = run(dir.path(), &["float", "--tag", "v1.3.0", "--execute"]);
    assert!(o.status.success(), "{}", err(&o));
    let v1_before = git_out(dir.path(), &["rev-parse", "v1^{commit}"]);

    git(
        dir.path(),
        &["commit", "-q", "--allow-empty", "-m", "hotfix"],
    );
    git(dir.path(), &["tag", "v1.2.1"]);
    let o = run(dir.path(), &["float", "--tag", "v1.2.1", "--execute"]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(
        err(&o).contains("Skipping v1 (1.3.0 is a newer"),
        "{}",
        err(&o)
    );
    assert_eq!(
        git_out(dir.path(), &["rev-parse", "v1^{commit}"]),
        v1_before
    );
    assert_eq!(
        git_out(dir.path(), &["rev-parse", "v1.2^{commit}"]),
        git_out(dir.path(), &["rev-parse", "v1.2.1^{commit}"])
    );
}

#[test]
fn float_pushes_when_a_branch_shares_the_floating_tag_name() {
    // Regression: `git push origin --force v1` failed with "src refspec v1
    // matches more than one" when a `v1` branch also existed.
    let dir = init_repo();
    let bare = add_origin(dir.path());
    git(dir.path(), &["tag", "v1.0.0"]);
    git(dir.path(), &["branch", "v1"]);
    git(dir.path(), &["push", "-q", "origin", "refs/heads/v1"]);

    let o = run(dir.path(), &["float", "--tag", "v1.0.0", "--execute"]);
    assert!(o.status.success(), "{}", err(&o));
    assert_eq!(tags(bare.path()), vec!["v1".to_string()]);
}

#[test]
fn float_refuses_a_tag_outside_tag_pattern() {
    let dir = init_repo();
    git(dir.path(), &["tag", "build-1.0.0"]);
    let o = run(dir.path(), &["float", "--tag", "build-1.0.0"]);
    assert!(!o.status.success());
    assert!(
        err(&o).contains("does not match tag-pattern"),
        "{}",
        err(&o)
    );
}

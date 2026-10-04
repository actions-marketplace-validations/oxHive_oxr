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
fn release_execute_without_yes_aborts_on_unconfirmed_prompt() {
    // `run` uses Output::output(), which closes stdin, so the confirmation
    // prompt reads EOF and treats it as "no" without hanging the test.
    let dir = init_repo();

    let o = run(dir.path(), &["release", "patch", "--execute"]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(err(&o).contains("[y/N]"), "{}", err(&o));
    assert!(err(&o).contains("aborted"), "{}", err(&o));

    let tags = String::from_utf8(
        Command::new("git")
            .args(["tag", "-l"])
            .current_dir(dir.path())
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert!(tags.is_empty(), "{tags}");
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
fn release_refuses_a_tag_that_already_exists() {
    // A tag oxr computes can only collide with one already in the repo if
    // that existing tag is invisible to version resolution (a custom
    // tag-pattern that doesn't match it) yet still occupies the tag-name
    // oxr's default template would render for a fresh bootstrap release.
    let dir = init_repo();
    std::fs::write(dir.path().join("oxr.toml"), "tag-pattern = \"^release-\"\n").unwrap();
    git(dir.path(), &["add", "oxr.toml"]);
    git(dir.path(), &["commit", "-q", "-m", "add config"]);
    git(dir.path(), &["tag", "v0.1.0"]);

    let o = run(dir.path(), &["current"]);
    assert!(out(&o).contains("latest_stable: none"), "{}", out(&o));

    let o = run(dir.path(), &["release", "patch", "--execute", "--yes"]);
    assert!(!o.status.success());
    assert!(err(&o).contains("already exists"), "{}", err(&o));
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

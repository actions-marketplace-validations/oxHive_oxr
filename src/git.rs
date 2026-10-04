use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

/// Runs `git <args>` in the given working directory and returns trimmed
/// stdout, or an error carrying git's stderr on non-zero exit.
fn run(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("failed to execute `git {}`", args.join(" ")))?;

    if !output.status.success() {
        bail!(
            "`git {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Like `run`, but lets git talk to the user directly (commit summary, push
/// progress). Its stdout is sent to our stderr so oxr's stdout stays data-only.
fn run_streaming(cwd: &Path, args: &[&str]) -> Result<()> {
    let status = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .stdout(std::io::stderr())
        .status()
        .with_context(|| format!("failed to execute `git {}`", args.join(" ")))?;

    if !status.success() {
        bail!("`git {}` failed ({status})", args.join(" "));
    }
    Ok(())
}

pub fn repo_root(cwd: &Path) -> Result<PathBuf> {
    let out = run(cwd, &["rev-parse", "--show-toplevel"])?;
    Ok(PathBuf::from(out))
}

/// Detects a shallow checkout (e.g. `actions/checkout`'s default
/// `fetch-depth: 1`), which silently breaks tag-based version resolution.
/// oxr must fail loudly here rather than compute a wrong version.
pub fn is_shallow(repo_root: &Path) -> Result<bool> {
    let out = run(repo_root, &["rev-parse", "--is-shallow-repository"])?;
    Ok(out == "true")
}

/// True if the working tree has staged, unstaged, or untracked changes.
pub fn is_dirty(repo_root: &Path) -> Result<bool> {
    let out = run(repo_root, &["status", "--porcelain"])?;
    Ok(!out.is_empty())
}

pub fn list_tags(repo_root: &Path) -> Result<Vec<String>> {
    let out = run(repo_root, &["tag", "-l"])?;
    Ok(out
        .lines()
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .collect())
}

pub fn current_branch(repo_root: &Path) -> Result<String> {
    run(repo_root, &["rev-parse", "--abbrev-ref", "HEAD"])
}

pub fn tag_exists(repo_root: &Path, name: &str) -> Result<bool> {
    Ok(list_tags(repo_root)?.iter().any(|t| t == name))
}

/// Resolves a tag (or any committish) to the commit sha it points at.
pub fn commit_of(repo_root: &Path, committish: &str) -> Result<String> {
    run(repo_root, &["rev-list", "-n", "1", committish])
}

pub fn create_tag(repo_root: &Path, name: &str, message: &str, sign: bool) -> Result<()> {
    let mut args = vec!["tag"];
    args.push(if sign { "-s" } else { "-a" });
    args.push(name);
    args.push("-m");
    args.push(message);
    run(repo_root, &args)?;
    Ok(())
}

/// Force-moves a tag to `target`, creating it if absent. Used for floating
/// tags (`v1`, `v1.2`) which are, by design, mutable pointers.
pub fn force_move_tag(
    repo_root: &Path,
    name: &str,
    target: &str,
    message: &str,
    sign: bool,
) -> Result<()> {
    let mut args = vec!["tag", "-f"];
    args.push(if sign { "-s" } else { "-a" });
    args.push(name);
    args.push("-m");
    args.push(message);
    args.push(target);
    run(repo_root, &args)?;
    Ok(())
}

pub fn push_tag(repo_root: &Path, name: &str, force: bool) -> Result<()> {
    let mut args = vec!["push", "origin"];
    if force {
        args.push("--force");
    }
    args.push(name);
    run_streaming(repo_root, &args)
}

pub fn push_current_branch(repo_root: &Path) -> Result<()> {
    run_streaming(repo_root, &["push"])
}

pub fn stage_and_commit(
    repo_root: &Path,
    paths: &[String],
    message: &str,
    sign: bool,
) -> Result<()> {
    let mut add_args = vec!["add"];
    add_args.extend(paths.iter().map(|s| s.as_str()));
    run(repo_root, &add_args)?;

    let mut commit_args = vec!["commit"];
    if sign {
        commit_args.push("-S");
    }
    commit_args.push("-m");
    commit_args.push(message);
    run_streaming(repo_root, &commit_args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as StdCommand;

    fn git(dir: &Path, args: &[&str]) {
        let status = StdCommand::new("git")
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

    #[test]
    fn run_reports_stderr_on_failure() {
        let dir = tempfile::tempdir().unwrap(); // not a git repo
        let err = repo_root(dir.path()).unwrap_err();
        assert!(err.to_string().contains("git rev-parse"));
    }

    #[test]
    fn repo_root_resolves_toplevel() {
        let dir = init_repo();
        let root = repo_root(dir.path()).unwrap();
        assert_eq!(
            root.canonicalize().unwrap(),
            dir.path().canonicalize().unwrap()
        );
    }

    #[test]
    fn is_shallow_false_for_a_normal_clone() {
        let dir = init_repo();
        assert!(!is_shallow(dir.path()).unwrap());
    }

    #[test]
    fn is_dirty_false_for_a_clean_checkout() {
        let dir = init_repo();
        assert!(!is_dirty(dir.path()).unwrap());
    }

    #[test]
    fn is_dirty_true_with_unstaged_changes() {
        let dir = init_repo();
        std::fs::write(dir.path().join("tracked"), "x").unwrap();
        git(dir.path(), &["add", "tracked"]);
        git(dir.path(), &["commit", "-q", "-m", "add tracked"]);

        std::fs::write(dir.path().join("tracked"), "y").unwrap();
        assert!(is_dirty(dir.path()).unwrap());
    }

    #[test]
    fn is_dirty_true_with_untracked_files() {
        let dir = init_repo();
        std::fs::write(dir.path().join("untracked"), "x").unwrap();
        assert!(is_dirty(dir.path()).unwrap());
    }

    #[test]
    fn is_shallow_true_for_a_shallow_clone() {
        let src = init_repo();
        git(src.path(), &["commit", "-q", "--allow-empty", "-m", "c2"]);

        let dst = tempfile::tempdir().unwrap();
        let src_url = format!("file://{}", src.path().display());
        git(dst.path(), &["clone", "-q", "--depth", "1", &src_url, "."]);

        assert!(is_shallow(dst.path()).unwrap());
    }

    #[test]
    fn list_tags_and_tag_exists() {
        let dir = init_repo();
        assert!(list_tags(dir.path()).unwrap().is_empty());
        assert!(!tag_exists(dir.path(), "v1.0.0").unwrap());

        create_tag(dir.path(), "v1.0.0", "release v1.0.0", false).unwrap();

        assert_eq!(list_tags(dir.path()).unwrap(), vec!["v1.0.0".to_string()]);
        assert!(tag_exists(dir.path(), "v1.0.0").unwrap());
    }

    #[test]
    fn commit_of_resolves_tag_to_head_sha() {
        let dir = init_repo();
        create_tag(dir.path(), "v1.0.0", "release v1.0.0", false).unwrap();
        let head = run(dir.path(), &["rev-parse", "HEAD"]).unwrap();
        assert_eq!(commit_of(dir.path(), "v1.0.0").unwrap(), head);
    }

    #[test]
    fn force_move_tag_creates_then_moves() {
        let dir = init_repo();
        let first = run(dir.path(), &["rev-parse", "HEAD"]).unwrap();
        force_move_tag(dir.path(), "v1", &first, "float", false).unwrap();
        assert_eq!(commit_of(dir.path(), "v1").unwrap(), first);

        git(dir.path(), &["commit", "-q", "--allow-empty", "-m", "c2"]);
        let second = run(dir.path(), &["rev-parse", "HEAD"]).unwrap();
        assert_ne!(first, second);

        force_move_tag(dir.path(), "v1", &second, "float", false).unwrap();
        assert_eq!(commit_of(dir.path(), "v1").unwrap(), second);
    }

    #[test]
    fn stage_and_commit_creates_a_commit() {
        let dir = init_repo();
        std::fs::write(dir.path().join("f.txt"), "hello").unwrap();
        stage_and_commit(dir.path(), &["f.txt".to_string()], "add f", false).unwrap();
        let log = run(dir.path(), &["log", "-1", "--pretty=%s"]).unwrap();
        assert_eq!(log, "add f");
    }

    #[test]
    fn push_tag_and_push_current_branch_reach_the_remote() {
        let bare = tempfile::tempdir().unwrap();
        git(bare.path(), &["init", "-q", "--bare"]);

        let dir = init_repo();
        git(dir.path(), &["config", "push.autoSetupRemote", "true"]);
        git(
            dir.path(),
            &["remote", "add", "origin", bare.path().to_str().unwrap()],
        );

        push_current_branch(dir.path()).unwrap();

        create_tag(dir.path(), "v1.0.0", "release v1.0.0", false).unwrap();
        push_tag(dir.path(), "v1.0.0", false).unwrap();

        let remote_tags = run(bare.path(), &["tag", "-l"]).unwrap();
        assert_eq!(remote_tags, "v1.0.0");
    }
}

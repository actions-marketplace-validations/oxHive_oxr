//! Tests for `install.sh` and the `action.yml` install step. `curl` and
//! `uname` are replaced with stubs on PATH so nothing touches the network
//! and the detected platform is fixed.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const TARGET: &str = "x86_64-unknown-linux-gnu";

fn repo_file(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(name)
}

fn write_exe(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn sh(dir: &Path, script: &str) {
    let status = Command::new("sh")
        .arg("-c")
        .arg(script)
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(status.success(), "{script}");
}

/// A scratch environment: a `bin/` of stubs prepended to PATH, and a
/// `release/` directory the curl stub serves assets from by file name.
struct Env {
    root: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let p = root.path();
        for d in ["bin", "release", "home", "pkg"] {
            std::fs::create_dir(p.join(d)).unwrap();
        }

        write_exe(
            &p.join("bin/uname"),
            "#!/bin/sh\ncase \"$1\" in -s) echo Linux ;; -m) echo x86_64 ;; *) echo Linux ;; esac\n",
        );
        // Serves `release/<basename of URL>`; exits 22 like `curl -f` on 404.
        write_exe(
            &p.join("bin/curl"),
            &format!(
                r#"#!/bin/sh
out=""; url=""
while [ $# -gt 0 ]; do
  case "$1" in
    -o) out="$2"; shift ;;
    http*) url="$1" ;;
  esac
  shift
done
echo "$url" >> "{log}"
src="{release}/$(basename "$url")"
[ -f "$src" ] || exit 22
cp "$src" "$out"
"#,
                log = p.join("curl.log").display(),
                release = p.join("release").display(),
            ),
        );

        write_exe(&p.join("pkg/oxr"), "#!/bin/sh\necho \"oxr 9.9.9\"\n");
        let asset = format!("oxr-{TARGET}.tar.gz");
        sh(p, &format!("tar -czf release/{asset} -C pkg oxr"));
        sh(
            p,
            &format!(
                "cd release && (sha256sum {asset} 2>/dev/null || shasum -a 256 {asset}) > checksums.txt"
            ),
        );
        Env { root }
    }

    fn path(&self) -> &Path {
        self.root.path()
    }

    fn command(&self, program: &str) -> Command {
        let mut cmd = Command::new(program);
        cmd.env_clear()
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.path().join("bin").display()),
            )
            .env("HOME", self.path().join("home"))
            .current_dir(self.path());
        cmd
    }

    fn install(&self, envs: &[(&str, &str)], args: &[&str]) -> Output {
        // Plain `sh`, as in `curl ... | sh`: dash on Debian/Ubuntu.
        self.command("sh")
            .arg(repo_file("install.sh"))
            .args(args)
            .envs(envs.iter().copied())
            .output()
            .unwrap()
    }

    fn urls(&self) -> String {
        std::fs::read_to_string(self.path().join("curl.log")).unwrap_or_default()
    }
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).to_string()
}

#[test]
fn installs_under_posix_sh_honoring_version_and_install_dir() {
    // Regression: `set -o pipefail` made dash abort on line 1, and the
    // README's VERSION/INSTALL_DIR variables were ignored.
    let env = Env::new();
    let dest = env.path().join("custom-bin");
    let o = env.install(
        &[
            ("VERSION", "v1.2.3"),
            ("INSTALL_DIR", dest.to_str().unwrap()),
        ],
        &[],
    );
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(dest.join("oxr").exists());
    assert!(env
        .urls()
        .contains(&format!("/releases/download/v1.2.3/oxr-{TARGET}.tar.gz")));
    assert!(String::from_utf8_lossy(&o.stdout).contains("oxr 9.9.9"));
}

#[test]
fn defaults_to_latest_into_home_local_bin() {
    let env = Env::new();
    let o = env.install(&[], &[]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(env.path().join("home/.local/bin/oxr").exists());
    assert!(env.urls().contains("/releases/latest/download/"));
}

#[test]
fn positional_version_still_works() {
    let env = Env::new();
    let o = env.install(&[], &["v0.2.0"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(env.urls().contains("/releases/download/v0.2.0/"));
}

#[test]
fn refuses_a_tarball_whose_checksum_does_not_match() {
    let env = Env::new();
    std::fs::write(
        env.path().join("release/checksums.txt"),
        format!("{}  oxr-{TARGET}.tar.gz\n", "0".repeat(64)),
    )
    .unwrap();
    let o = env.install(&[], &[]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("checksum mismatch"), "{}", stderr(&o));
    assert!(!env.path().join("home/.local/bin/oxr").exists());
}

#[test]
fn missing_checksums_fail_unless_explicitly_skipped() {
    let env = Env::new();
    std::fs::remove_file(env.path().join("release/checksums.txt")).unwrap();

    let o = env.install(&[], &[]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("OXR_SKIP_CHECKSUM"), "{}", stderr(&o));

    let o = env.install(&[("OXR_SKIP_CHECKSUM", "1")], &[]);
    assert!(o.status.success(), "{}", stderr(&o));
}

/// The body of the action's `run: |` block, dedented.
fn action_run_script() -> String {
    let text = std::fs::read_to_string(repo_file("action.yml")).unwrap();
    let lines = text.lines().skip_while(|l| l.trim() != "run: |").skip(1);
    let first = lines.clone().find(|l| !l.trim().is_empty()).unwrap();
    let indent = first.len() - first.trim_start().len();
    lines
        .take_while(|l| l.trim().is_empty() || l.len() - l.trim_start().len() >= indent)
        .map(|l| l.get(indent..).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn action_run_step_interpolates_no_expressions() {
    // Everything from the workflow context must arrive via `env:`.
    assert!(!action_run_script().contains("${{"));
}

#[test]
fn action_installs_the_release_matching_its_own_cargo_version() {
    // Regression: the action downloaded the release named after
    // `github.action_ref`, so SHA pins, `@main`, and `@v1` (which has no
    // release of its own) all 404'd.
    let env = Env::new();
    let runner_temp = env.path().join("runner");
    std::fs::create_dir(&runner_temp).unwrap();
    let github_path = env.path().join("github_path");

    let o = env
        .command("bash")
        .arg("-c")
        .arg(action_run_script())
        .env("OXR_VERSION_INPUT", "")
        .env("GITHUB_ACTION_PATH", env!("CARGO_MANIFEST_DIR"))
        .env("RUNNER_TEMP", &runner_temp)
        .env("GITHUB_PATH", &github_path)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", stderr(&o));

    let expected = format!(
        "/releases/download/v{}/oxr-{TARGET}.tar.gz",
        env!("CARGO_PKG_VERSION")
    );
    assert!(env.urls().contains(&expected), "{}", env.urls());
    let bin_dir = runner_temp.join("oxr-bin");
    assert!(bin_dir.join("oxr").exists());
    assert_eq!(
        std::fs::read_to_string(&github_path).unwrap().trim(),
        bin_dir.to_str().unwrap()
    );
}

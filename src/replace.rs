use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};
use regex::Regex;
use semver::Version;

use crate::config::Replacement;
use crate::template;

/// The planned end state of one file touched by `[[pre-release-replacements]]`.
#[derive(Debug)]
pub struct FileChange {
    /// The file as configured, relative to the repo root.
    pub file: String,
    path: PathBuf,
    original: String,
    updated: String,
}

impl FileChange {
    /// False when every replacement rendered text identical to what was
    /// already there (e.g. a `{{major}}`-only reference on a patch release).
    pub fn changed(&self) -> bool {
        self.original != self.updated
    }
}

/// Computes every file's post-replacement contents in memory without
/// writing anything, erroring if any entry is invalid or its `search`
/// doesn't match exactly `exactly` times. Validating all entries up front
/// means a bad second entry can't leave the first file half-rewritten.
/// Entries targeting the same file apply in order to its evolving contents.
pub fn plan(
    repo_root: &Path,
    replacements: &[Replacement],
    version: &Version,
) -> Result<Vec<FileChange>> {
    let mut changes: Vec<FileChange> = Vec::new();

    for r in replacements {
        let idx = match changes.iter().position(|c| c.file == r.file) {
            Some(i) => i,
            None => {
                let path = resolve_in_repo(repo_root, &r.file)?;
                let contents = std::fs::read_to_string(&path)
                    .with_context(|| format!("reading {}", path.display()))?;
                changes.push(FileChange {
                    file: r.file.clone(),
                    path,
                    original: contents.clone(),
                    updated: contents,
                });
                changes.len() - 1
            }
        };
        let change = &mut changes[idx];

        let re = Regex::new(&r.search)
            .with_context(|| format!("invalid search regex for {}", r.file))?;
        let match_count = re.find_iter(&change.updated).count();
        if match_count != r.exactly {
            bail!(
                "{}: expected search pattern to match exactly {} time(s), found {}",
                r.file,
                r.exactly,
                match_count
            );
        }

        let replacement = template::render(&brace_backrefs(&r.replace), version);
        change.updated = re
            .replace_all(&change.updated, replacement.as_str())
            .into_owned();
    }

    Ok(changes)
}

/// Writes every changed file planned by `plan`.
pub fn write(changes: &[FileChange]) -> Result<()> {
    for c in changes.iter().filter(|c| c.changed()) {
        std::fs::write(&c.path, &c.updated)
            .with_context(|| format!("writing {}", c.path.display()))?;
    }
    Ok(())
}

/// Resolves `file` against the repo root, refusing absolute paths, `..`
/// components, and symlinks that lead outside the repository.
fn resolve_in_repo(repo_root: &Path, file: &str) -> Result<PathBuf> {
    let rel = Path::new(file);
    if rel
        .components()
        .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        bail!("{file}: pre-release-replacements paths must be relative and stay inside the repository");
    }

    let path = repo_root.join(rel);
    let root = repo_root
        .canonicalize()
        .with_context(|| format!("resolving {}", repo_root.display()))?;
    let resolved = path
        .canonicalize()
        .with_context(|| format!("reading {}", path.display()))?;
    if !resolved.starts_with(&root) {
        bail!(
            "{file}: resolves outside the repository ({})",
            resolved.display()
        );
    }
    Ok(path)
}

/// Rewrites `$name` group references in a `replace` template to the
/// unambiguous `${name}` form *before* placeholders are rendered.
/// Otherwise `$1{{version}}` renders to `$11.2.3`, which the regex crate
/// reads as group `11` and silently expands to nothing. `$$` and `${...}`
/// pass through untouched.
fn brace_backrefs(replace: &str) -> String {
    let mut out = String::with_capacity(replace.len());
    let mut chars = replace.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('$') => {
                chars.next();
                out.push_str("$$");
            }
            Some(&n) if n == '_' || n.is_ascii_alphanumeric() => {
                let mut name = String::new();
                while let Some(&n) = chars.peek() {
                    if n == '_' || n.is_ascii_alphanumeric() {
                        name.push(n);
                        chars.next();
                    } else {
                        break;
                    }
                }
                out.push_str("${");
                out.push_str(&name);
                out.push('}');
            }
            _ => out.push('$'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn replacement(file: &str, search: &str, replace: &str, exactly: usize) -> Replacement {
        Replacement {
            file: file.to_string(),
            search: search.to_string(),
            replace: replace.to_string(),
            exactly,
        }
    }

    fn apply(dir: &Path, rs: &[Replacement], version: &str) -> Result<Vec<FileChange>> {
        let changes = plan(dir, rs, &Version::parse(version).unwrap())?;
        write(&changes)?;
        Ok(changes)
    }

    fn read(dir: &Path, file: &str) -> String {
        std::fs::read_to_string(dir.join(file)).unwrap()
    }

    const VERSION_SEARCH: &str = r#""version": "[^"]+""#;
    const VERSION_REPLACE: &str = r#""version": "{{version}}""#;

    #[test]
    fn replaces_version_field() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("plugin.json"),
            r#"{"name": "foo", "version": "0.1.0"}"#,
        )
        .unwrap();
        let r = replacement("plugin.json", VERSION_SEARCH, VERSION_REPLACE, 1);
        apply(dir.path(), &[r], "0.2.0").unwrap();
        assert_eq!(
            read(dir.path(), "plugin.json"),
            r#"{"name": "foo", "version": "0.2.0"}"#
        );
    }

    #[test]
    fn plan_reports_a_typo_in_search_without_writing_anything() {
        let dir = tempfile::tempdir().unwrap();
        let original = r#"{"name": "foo", "version": "0.1.0"}"#;
        std::fs::write(dir.path().join("plugin.json"), original).unwrap();
        // "varsion" instead of "version": matches nothing.
        let r = replacement(
            "plugin.json",
            r#""varsion": "[^"]+""#,
            r#""varsion": "{{version}}""#,
            1,
        );

        let err = plan(dir.path(), &[r], &Version::parse("0.2.0").unwrap()).unwrap_err();
        assert!(err.to_string().contains("found 0"), "{err}");
        assert_eq!(read(dir.path(), "plugin.json"), original);
    }

    #[test]
    fn a_failing_later_entry_leaves_earlier_files_untouched() {
        // Regression: entries used to be applied one at a time, so a bad
        // second entry left the first file rewritten on disk.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.json"), r#""version": "1.0.0""#).unwrap();
        std::fs::write(dir.path().join("b.json"), "no version here").unwrap();
        let rs = [
            replacement("a.json", VERSION_SEARCH, VERSION_REPLACE, 1),
            replacement("b.json", VERSION_SEARCH, VERSION_REPLACE, 1),
        ];

        let err = apply(dir.path(), &rs, "1.0.1").unwrap_err();
        assert!(err.to_string().contains("b.json"), "{err}");
        assert_eq!(read(dir.path(), "a.json"), r#""version": "1.0.0""#);
    }

    #[test]
    fn unchanged_contents_are_reported_as_not_changed() {
        // Regression: a `{{major}}`-only reference renders identically on a
        // patch release; it used to be committed anyway, failing the commit.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("README.md"), "uses: oxhive/oxr@v1\n").unwrap();
        let r = replacement("README.md", r"oxhive/oxr@v\d+", "oxhive/oxr@v{{major}}", 1);
        let changes = apply(dir.path(), &[r], "1.0.1").unwrap();
        assert!(!changes[0].changed());
    }

    #[test]
    fn entries_for_the_same_file_chain() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f"), "a=0.1.0 b=0.1.0").unwrap();
        let rs = [
            replacement("f", r"a=[0-9.]+", "a={{version}}", 1),
            replacement("f", r"b=[0-9.]+", "b={{version}}", 1),
        ];
        apply(dir.path(), &rs, "0.2.0").unwrap();
        assert_eq!(read(dir.path(), "f"), "a=0.2.0 b=0.2.0");
    }

    #[test]
    fn errors_on_match_count_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("plugin.json"), "no version field here").unwrap();
        let r = replacement("plugin.json", VERSION_SEARCH, VERSION_REPLACE, 1);
        let err = apply(dir.path(), &[r], "0.2.0").unwrap_err();
        assert!(err.to_string().contains("expected search pattern"));
    }

    #[test]
    fn supports_regex_backreferences() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("plugin.json"), "prefix-1.0.0-suffix").unwrap();
        let r = replacement(
            "plugin.json",
            r"prefix-([0-9.]+)-suffix",
            "prefix-{{version}}-suffix-$1",
            1,
        );
        apply(dir.path(), &[r], "2.0.0").unwrap();
        assert_eq!(read(dir.path(), "plugin.json"), "prefix-2.0.0-suffix-1.0.0");
    }

    #[test]
    fn backreference_directly_before_version_placeholder() {
        // Regression: `$1{{version}}` rendered to `$10.1.1`, which the regex
        // crate read as group 10 and replaced with nothing.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f.txt"), "version = \"0.1.0\"\n").unwrap();
        let r = replacement("f.txt", r#"(version = ")[^"]+""#, r#"$1{{version}}""#, 1);
        apply(dir.path(), &[r], "0.1.1").unwrap();
        assert_eq!(read(dir.path(), "f.txt"), "version = \"0.1.1\"\n");
    }

    #[test]
    fn brace_backrefs_leaves_escapes_and_braced_forms_alone() {
        assert_eq!(brace_backrefs("$1x"), "${1x}");
        assert_eq!(brace_backrefs("$1{{version}}"), "${1}{{version}}");
        assert_eq!(brace_backrefs("${1}v"), "${1}v");
        assert_eq!(brace_backrefs("$$1"), "$$1");
        assert_eq!(brace_backrefs("cost: $"), "cost: $");
        assert_eq!(brace_backrefs("$name-x"), "${name}-x");
    }

    #[test]
    fn rejects_paths_outside_the_repo() {
        let outer = tempfile::tempdir().unwrap();
        let repo = outer.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        std::fs::write(outer.path().join("secret"), "version = 1").unwrap();
        let v = Version::parse("1.0.0").unwrap();

        for file in ["../secret", "/etc/hostname"] {
            let r = replacement(file, "version", "version", 1);
            let err = plan(&repo, &[r], &v).unwrap_err();
            assert!(err.to_string().contains("inside the repository"), "{err}");
        }

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outer.path().join("secret"), repo.join("link")).unwrap();
            let r = replacement("link", "version", "version", 1);
            let err = plan(&repo, &[r], &v).unwrap_err();
            assert!(err.to_string().contains("outside the repository"), "{err}");
        }
    }
}

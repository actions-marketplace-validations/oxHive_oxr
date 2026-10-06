use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

fn default_true() -> bool {
    true
}

fn default_tag_name() -> String {
    "v{{version}}".to_string()
}

fn default_tag_pattern() -> String {
    r"^v\d+\.\d+\.\d+".to_string()
}

fn default_commit_message() -> String {
    "chore: release v{{version}}".to_string()
}

fn default_major_tag_name() -> String {
    "v{{major}}".to_string()
}

fn default_minor_tag_name() -> String {
    "v{{major}}.{{minor}}".to_string()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct Config {
    pub sign_commit: bool,
    pub sign_tag: bool,
    #[serde(default = "default_true")]
    pub push: bool,
    #[serde(default = "default_true")]
    pub tag: bool,
    #[serde(default = "default_tag_name")]
    pub tag_name: String,
    #[serde(default = "default_tag_pattern")]
    pub tag_pattern: String,
    #[serde(default = "default_commit_message")]
    pub pre_release_commit_message: String,
    #[serde(default)]
    pub float_tags: FloatTags,
    #[serde(default)]
    pub pre_release_replacements: Vec<Replacement>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            sign_commit: false,
            sign_tag: false,
            push: true,
            tag: true,
            tag_name: default_tag_name(),
            tag_pattern: default_tag_pattern(),
            pre_release_commit_message: default_commit_message(),
            float_tags: FloatTags::default(),
            pre_release_replacements: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct FloatTags {
    #[serde(default = "default_true")]
    pub major: bool,
    #[serde(default)]
    pub minor: bool,
    #[serde(default = "default_major_tag_name")]
    pub major_tag_name: String,
    #[serde(default = "default_minor_tag_name")]
    pub minor_tag_name: String,
}

impl Default for FloatTags {
    fn default() -> Self {
        FloatTags {
            major: true,
            minor: false,
            major_tag_name: default_major_tag_name(),
            minor_tag_name: default_minor_tag_name(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Replacement {
    pub file: String,
    pub search: String,
    pub replace: String,
    pub exactly: usize,
}

/// Unknown keys are rejected rather than ignored: a typo (`sign-tags`) or a
/// cargo-release-only field (`tag-prefix`) must not silently fall back to
/// a default.
///
/// Loads `oxr.toml` from the repo root, falling back to `release.toml` for
/// anyone coming from cargo-release out of habit. `oxr.toml` is checked
/// first (and is what `oxr init` writes) so the unambiguous name wins by
/// default — the two schemas share vocabulary but aren't compatible, and a
/// bare `release.toml` reads as "this is a cargo-release config" to anyone
/// who knows that tool. Neither file existing is not an error: repos with
/// no manifest-embedded version and no floating-tag needs can run oxr on
/// defaults alone.
pub fn load(repo_root: &Path) -> Result<Config> {
    for name in ["oxr.toml", "release.toml"] {
        let path = repo_root.join(name);
        if path.exists() {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            let config: Config =
                toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
            return Ok(config);
        }
    }
    Ok(Config::default())
}

/// Scaffold written by `oxr init`. Fully commented on purpose: oxr behaves
/// identically to having no config file at all until a setting is
/// uncommented, so `init` can never silently change release behavior
/// (e.g. activating a `pre-release-replacements` entry against a file that
/// doesn't exist yet would break the next release).
pub const SCAFFOLD: &str = r#"# oxr configuration.
#
# The current version is always read from git tags -- nothing in this file
# is a version field. Uncomment and edit only the settings you want to
# change from their defaults.

# sign-commit = false
# sign-tag = false
# push = true
# tag = true
# tag-name = "v{{version}}"
# tag-pattern = "^v\\d+\\.\\d+\\.\\d+"
# pre-release-commit-message = "chore: release v{{version}}"

# [float-tags]
# major = true
# minor = false
# major-tag-name = "v{{major}}"
# minor-tag-name = "v{{major}}.{{minor}}"

# Keep an embedded version string in sync on every release, e.g. for a
# manifest file such as .claude-plugin/plugin.json:
#
# [[pre-release-replacements]]
# file = ".claude-plugin/plugin.json"
# search = "\"version\": \"[^\"]+\""
# replace = "\"version\": \"{{version}}\""
# exactly = 1
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_spec() {
        let c = Config::default();
        assert!(!c.sign_commit);
        assert!(!c.sign_tag);
        assert!(c.push);
        assert!(c.tag);
        assert_eq!(c.tag_name, "v{{version}}");
        assert_eq!(c.tag_pattern, r"^v\d+\.\d+\.\d+");
        assert_eq!(c.pre_release_commit_message, "chore: release v{{version}}");
        assert!(c.float_tags.major);
        assert!(!c.float_tags.minor);
        assert_eq!(c.float_tags.major_tag_name, "v{{major}}");
        assert_eq!(c.float_tags.minor_tag_name, "v{{major}}.{{minor}}");
        assert!(c.pre_release_replacements.is_empty());
    }

    #[test]
    fn parses_full_example_from_spec() {
        let toml_text = r#"
sign-commit = false
sign-tag = false
push = true
tag = true
tag-name = "v{{version}}"
tag-pattern = "^v\\d+\\.\\d+\\.\\d+"
pre-release-commit-message = "chore: release v{{version}}"

[float-tags]
major = true
minor = false
major-tag-name = "v{{major}}"
minor-tag-name = "v{{major}}.{{minor}}"

[[pre-release-replacements]]
file = ".claude-plugin/plugin.json"
search = "\"version\": \"[^\"]+\""
replace = "\"version\": \"{{version}}\""
exactly = 1
"#;
        let c: Config = toml::from_str(toml_text).unwrap();
        assert_eq!(c.pre_release_replacements.len(), 1);
        assert_eq!(c.pre_release_replacements[0].exactly, 1);
    }

    #[test]
    fn missing_config_file_uses_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let c = load(dir.path()).unwrap();
        assert_eq!(c.tag_name, "v{{version}}");
    }

    #[test]
    fn prefers_oxr_toml_over_release_toml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("oxr.toml"), "push = false\n").unwrap();
        std::fs::write(dir.path().join("release.toml"), "push = true\n").unwrap();
        let c = load(dir.path()).unwrap();
        assert!(!c.push);
    }

    #[test]
    fn falls_back_to_release_toml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("release.toml"), "push = false\n").unwrap();
        let c = load(dir.path()).unwrap();
        assert!(!c.push);
    }

    #[test]
    fn scaffold_parses_and_matches_defaults() {
        let c: Config = toml::from_str(SCAFFOLD).unwrap();
        let d = Config::default();
        assert_eq!(c.sign_commit, d.sign_commit);
        assert_eq!(c.sign_tag, d.sign_tag);
        assert_eq!(c.push, d.push);
        assert_eq!(c.tag, d.tag);
        assert_eq!(c.tag_name, d.tag_name);
        assert_eq!(c.tag_pattern, d.tag_pattern);
        assert_eq!(c.pre_release_commit_message, d.pre_release_commit_message);
        assert_eq!(c.float_tags.major, d.float_tags.major);
        assert_eq!(c.float_tags.minor, d.float_tags.minor);
        assert!(c.pre_release_replacements.is_empty());
    }

    #[test]
    fn scaffold_written_by_init_round_trips_through_load() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("oxr.toml"), SCAFFOLD).unwrap();
        let c = load(dir.path()).unwrap();
        assert_eq!(c.tag_name, "v{{version}}");
    }

    #[test]
    fn unknown_keys_are_rejected() {
        // Regression: typos and cargo-release-only keys were silently ignored.
        for text in [
            "pushh = false\n",
            "tag-prefix = \"v\"\n",
            "[float-tags]\nmajr = false\n",
            "[[pre-release-replacements]]\nfile = \"f\"\nsearch = \"x\"\nreplace = \"y\"\nexactly = 1\nmin = 1\n",
        ] {
            let err = toml::from_str::<Config>(text).unwrap_err();
            assert!(err.to_string().contains("unknown field"), "{text}: {err}");
        }
    }
}

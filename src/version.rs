use std::sync::LazyLock;

use anyhow::{bail, Context, Result};
use regex::Regex;
use semver::Version;

static VERSION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?").unwrap()
});

/// The two resolution queries over tag state. Kept distinct per spec: they
/// answer different questions and must never be conflated.
#[derive(Debug, Clone, Default)]
pub struct Resolution {
    /// Highest semver tag with no pre-release component.
    pub latest_stable: Option<Version>,
    /// Highest-precedence tag across ALL matching tags, stable or not.
    pub latest_overall: Option<Version>,
}

impl Resolution {
    /// The active pre-release train, if `latest_overall` carries a
    /// pre-release component. Whether a train is active is entirely derived
    /// from tag state, never stored.
    pub fn active_train(&self) -> Option<&Version> {
        self.latest_overall.as_ref().filter(|v| !v.pre.is_empty())
    }

    pub fn stable_or_zero(&self) -> Version {
        self.latest_stable.clone().unwrap_or_else(zero_version)
    }
}

pub fn zero_version() -> Version {
    Version::new(0, 0, 0)
}

/// Extracts the semver-parseable substring from a tag name, e.g.
/// `v1.2.3-rc.1` -> `1.2.3-rc.1`. `tag-pattern` only filters which tags are
/// considered; this is what actually strips a literal prefix like `v`.
pub fn extract_version(tag: &str) -> Option<Version> {
    let m = VERSION_RE.find(tag)?;
    Version::parse(m.as_str()).ok()
}

/// Compiles the configured `tag-pattern`, naming the setting on failure.
pub fn compile_pattern(tag_pattern: &str) -> Result<Regex> {
    Regex::new(tag_pattern).with_context(|| format!("invalid tag-pattern '{tag_pattern}'"))
}

/// Every version carried by a tag matching `tag_pattern`. Errors on a
/// matching tag that isn't valid semver rather than silently skipping it.
pub fn matching_versions(tags: &[String], tag_pattern: &str) -> Result<Vec<Version>> {
    let pattern = compile_pattern(tag_pattern)?;
    tags.iter()
        .filter(|tag| pattern.is_match(tag))
        .map(|tag| match extract_version(tag) {
            Some(v) => Ok(v),
            None => bail!(
                "tag '{tag}' matches tag-pattern '{tag_pattern}' but is not a valid semver version"
            ),
        })
        .collect()
}

/// Resolves `latest_stable` and `latest_overall` from raw tag names, using a
/// real semver-aware comparison (not lexicographic, not git's native tag
/// sort) as required by the spec.
pub fn resolve(tags: &[String], tag_pattern: &str) -> Result<Resolution> {
    let mut latest_stable: Option<Version> = None;
    let mut latest_overall: Option<Version> = None;

    for version in matching_versions(tags, tag_pattern)? {
        if version.pre.is_empty() && latest_stable.as_ref().is_none_or(|cur| version > *cur) {
            latest_stable = Some(version.clone());
        }
        if latest_overall.as_ref().is_none_or(|cur| version > *cur) {
            latest_overall = Some(version);
        }
    }

    Ok(Resolution {
        latest_stable,
        latest_overall,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    const DEFAULT_PATTERN: &str = r"^v\d+\.\d+\.\d+";

    #[test]
    fn bootstrap_no_tags() {
        let r = resolve(&[], DEFAULT_PATTERN).unwrap();
        assert!(r.latest_stable.is_none());
        assert!(r.latest_overall.is_none());
        assert!(r.active_train().is_none());
        assert_eq!(r.stable_or_zero(), zero_version());
    }

    #[test]
    fn stable_ignores_prereleases() {
        let r = resolve(&tags(&["v1.0.0", "v1.1.0", "v1.2.0-rc.1"]), DEFAULT_PATTERN).unwrap();
        assert_eq!(r.latest_stable, Some(Version::parse("1.1.0").unwrap()));
        assert_eq!(
            r.latest_overall,
            Some(Version::parse("1.2.0-rc.1").unwrap())
        );
        assert!(r.active_train().is_some());
    }

    #[test]
    fn no_active_train_when_overall_is_stable() {
        let r = resolve(&tags(&["v1.0.0", "v1.1.0"]), DEFAULT_PATTERN).unwrap();
        assert!(r.active_train().is_none());
    }

    #[test]
    fn tag_pattern_filters_non_version_tags() {
        let r = resolve(
            &tags(&["v1.0.0", "checkpoint-1", "release-marker"]),
            DEFAULT_PATTERN,
        )
        .unwrap();
        assert_eq!(r.latest_stable, Some(Version::parse("1.0.0").unwrap()));
    }

    #[test]
    fn semver_precedence_not_lexicographic() {
        // Lexicographically "v1.9.0" > "v1.10.0", but semver says otherwise.
        let r = resolve(&tags(&["v1.9.0", "v1.10.0"]), DEFAULT_PATTERN).unwrap();
        assert_eq!(r.latest_stable, Some(Version::parse("1.10.0").unwrap()));
    }

    #[test]
    fn orphaned_higher_prerelease_still_wins_overall() {
        // Documented edge case: an earlier planning cycle's shelved train
        // still resolves as active since major.minor.patch outranks 1.4.2.
        let r = resolve(&tags(&["v1.4.2", "v1.5.0-rc.1"]), DEFAULT_PATTERN).unwrap();
        assert_eq!(r.latest_stable, Some(Version::parse("1.4.2").unwrap()));
        assert_eq!(
            r.active_train(),
            Some(&Version::parse("1.5.0-rc.1").unwrap())
        );
    }

    #[test]
    fn extract_strips_v_prefix() {
        assert_eq!(
            extract_version("v1.2.3-rc.1"),
            Some(Version::parse("1.2.3-rc.1").unwrap())
        );
    }

    #[test]
    fn invalid_tag_pattern_error_names_the_setting() {
        let err = resolve(&tags(&["v1.0.0"]), "^v(").unwrap_err();
        assert!(err.to_string().contains("invalid tag-pattern"), "{err}");
    }
}

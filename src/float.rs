use anyhow::{bail, Result};
use semver::Version;

use crate::config::FloatTags;
use crate::template;
use crate::version::extract_version;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FloatPlan {
    pub version: Version,
    /// Floating tags to force-move to the target, e.g. `["v1"]`.
    /// A major bump only ever adds a new entry here (e.g. `v2`); the
    /// previous major's floating tag is never included, never touched.
    pub tags: Vec<String>,
    /// Floating tags left alone because a newer stable release already
    /// exists in their line, paired with that newer version.
    pub skipped: Vec<(String, Version)>,
}

/// Computes which floating tags should move to `tag`, or hard-refuses if
/// `tag` carries a pre-release identifier. This safety check must live in
/// the binary itself, not just in CI trigger wiring — floating tags exist
/// so external consumers get trusted, stable code.
///
/// `known` is every version resolved from tags matching `tag-pattern`. A
/// floating tag only moves if `tag` is the newest stable release in that
/// tag's line, so floating a hotfix for an older minor (`v1.2.1` after
/// `v1.3.0`) can never drag `v1` backwards.
pub fn plan(tag: &str, float_tags: &FloatTags, known: &[Version]) -> Result<FloatPlan> {
    let version = extract_version(tag)
        .ok_or_else(|| anyhow::anyhow!("'{tag}' does not contain a parseable semver version"))?;

    if !version.pre.is_empty() {
        bail!(
            "refusing to float a pre-release tag ({version}): floating tags must only ever \
             point at stable releases"
        );
    }

    let newest_in_line = |same_line: &dyn Fn(&Version) -> bool| {
        known
            .iter()
            .filter(|v| v.pre.is_empty() && same_line(v))
            .filter(|v| v.cmp_precedence(&version).is_gt())
            .max_by(|a, b| a.cmp_precedence(b))
            .cloned()
    };

    let mut tags = Vec::new();
    let mut skipped = Vec::new();
    let mut consider = |name: String, newer: Option<Version>| match newer {
        Some(newer) => skipped.push((name, newer)),
        None => tags.push(name),
    };

    if float_tags.major {
        consider(
            template::render(&float_tags.major_tag_name, &version),
            newest_in_line(&|v| v.major == version.major),
        );
    }
    if float_tags.minor {
        consider(
            template::render(&float_tags.minor_tag_name, &version),
            newest_in_line(&|v| v.major == version.major && v.minor == version.minor),
        );
    }

    Ok(FloatPlan {
        version,
        tags,
        skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn float_tags(major: bool, minor: bool) -> FloatTags {
        FloatTags {
            major,
            minor,
            major_tag_name: "v{{major}}".to_string(),
            minor_tag_name: "v{{major}}.{{minor}}".to_string(),
        }
    }

    fn versions(list: &[&str]) -> Vec<Version> {
        list.iter().map(|s| Version::parse(s).unwrap()).collect()
    }

    #[test]
    fn refuses_prerelease_tag() {
        let err = plan("v1.5.0-rc.1", &float_tags(true, false), &[]).unwrap_err();
        assert!(err
            .to_string()
            .contains("refusing to float a pre-release tag"));
    }

    #[test]
    fn major_only_produces_major_tag() {
        let p = plan("v1.4.3", &float_tags(true, false), &versions(&["1.4.3"])).unwrap();
        assert_eq!(p.tags, vec!["v1".to_string()]);
    }

    #[test]
    fn major_and_minor_both_enabled() {
        let p = plan("v1.4.3", &float_tags(true, true), &versions(&["1.4.3"])).unwrap();
        assert_eq!(p.tags, vec!["v1".to_string(), "v1.4".to_string()]);
    }

    #[test]
    fn major_bump_never_references_previous_major() {
        // v2.0.0 plan must only ever mention v2, never touch v1.
        let known = versions(&["1.9.0", "2.0.0"]);
        let p = plan("v2.0.0", &float_tags(true, false), &known).unwrap();
        assert_eq!(p.tags, vec!["v2".to_string()]);
        assert!(!p.tags.iter().any(|t| t == "v1"));
    }

    #[test]
    fn hotfix_on_an_older_minor_does_not_move_major_backwards() {
        // Regression: floating v1.2.1 after v1.3.0 used to move v1 back.
        let known = versions(&["1.2.0", "1.3.0", "1.2.1"]);
        let p = plan("v1.2.1", &float_tags(true, true), &known).unwrap();
        assert_eq!(p.tags, vec!["v1.2".to_string()]);
        assert_eq!(
            p.skipped,
            vec![("v1".to_string(), Version::parse("1.3.0").unwrap())]
        );
    }

    #[test]
    fn newer_prerelease_in_line_does_not_block_floating() {
        let known = versions(&["1.3.0", "1.4.0-rc.1"]);
        let p = plan("v1.3.0", &float_tags(true, false), &known).unwrap();
        assert_eq!(p.tags, vec!["v1".to_string()]);
    }
}

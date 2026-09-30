use anyhow::{anyhow, bail, Context, Result};
use clap::ValueEnum;
use semver::{BuildMetadata, Prerelease, Version};

use crate::version::Resolution;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lowercase")]
pub enum Level {
    Patch,
    Minor,
    Major,
    Stable,
    Alpha,
    Beta,
    Rc,
}

/// The `--for <major|minor>` CLI flag. Deliberately excludes `patch`: it's
/// already the implicit default when starting a fresh pre-release train.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lowercase")]
pub enum ForTarget {
    Major,
    Minor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BumpTarget {
    Major,
    Minor,
    Patch,
}

impl From<ForTarget> for BumpTarget {
    fn from(f: ForTarget) -> Self {
        match f {
            ForTarget::Major => BumpTarget::Major,
            ForTarget::Minor => BumpTarget::Minor,
        }
    }
}

fn bump(base: &Version, target: BumpTarget) -> Version {
    match target {
        BumpTarget::Major => Version::new(base.major + 1, 0, 0),
        BumpTarget::Minor => Version::new(base.major, base.minor + 1, 0),
        BumpTarget::Patch => Version::new(base.major, base.minor, base.patch + 1),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Stage {
    Alpha,
    Beta,
    Rc,
}

impl Stage {
    fn name(self) -> &'static str {
        match self {
            Stage::Alpha => "alpha",
            Stage::Beta => "beta",
            Stage::Rc => "rc",
        }
    }

    fn from_level(level: Level) -> Option<Self> {
        match level {
            Level::Alpha => Some(Stage::Alpha),
            Level::Beta => Some(Stage::Beta),
            Level::Rc => Some(Stage::Rc),
            _ => None,
        }
    }

    fn from_prerelease(version: &Version) -> Result<Self> {
        let stage_name = version.pre.as_str().split('.').next().unwrap_or("");
        match stage_name {
            "alpha" => Ok(Stage::Alpha),
            "beta" => Ok(Stage::Beta),
            "rc" => Ok(Stage::Rc),
            other => bail!(
                "active train '{version}' has an unrecognized pre-release stage '{other}' (expected alpha, beta, or rc)"
            ),
        }
    }
}

fn current_counter(version: &Version) -> Result<u64> {
    let mut parts = version.pre.as_str().split('.');
    parts.next(); // stage name
    match parts.next() {
        Some(n) => n
            .parse::<u64>()
            .with_context(|| format!("pre-release counter in '{version}' is not numeric")),
        None => Ok(0),
    }
}

/// Computes the next version for `level`, given the repo's current tag
/// resolution and an optional `--for` override. Pure and git-independent
/// so the full release decision table can be tested without a repo.
pub fn next_version(
    resolution: &Resolution,
    level: Level,
    for_target: Option<ForTarget>,
) -> Result<Version> {
    // No stable release yet: the first stable is always a minor bump from
    // 0.0.0, even if pre-release tags already exist (e.g. `v0.1.0-rc.1`).
    let bootstrap = resolution.latest_stable.is_none();

    match level {
        Level::Patch | Level::Minor | Level::Major => {
            if for_target.is_some() {
                bail!("--for is only valid with the alpha, beta, or rc levels");
            }
            let base = resolution.stable_or_zero();
            let target = if bootstrap {
                BumpTarget::Minor
            } else {
                match level {
                    Level::Patch => BumpTarget::Patch,
                    Level::Minor => BumpTarget::Minor,
                    Level::Major => BumpTarget::Major,
                    _ => unreachable!(),
                }
            };
            let next = bump(&base, target);
            if bootstrap {
                if let Some(train) = resolution.active_train() {
                    if next < *train {
                        bail!(
                            "no stable release exists yet, and the active train ({train}) \
                             targets a higher version than {next}; finalize it with \
                             `oxr release stable` instead"
                        );
                    }
                }
            }
            Ok(next)
        }

        Level::Stable => {
            if for_target.is_some() {
                bail!("--for is only valid with the alpha, beta, or rc levels");
            }
            let train = resolution.active_train().ok_or_else(|| {
                anyhow!(
                    "no active pre-release train to finalize: `oxr release stable` requires \
                     the latest tag overall to carry a pre-release component"
                )
            })?;
            let mut finalized = train.clone();
            finalized.pre = Prerelease::EMPTY;
            finalized.build = BuildMetadata::EMPTY;
            Ok(finalized)
        }

        Level::Alpha | Level::Beta | Level::Rc => {
            let stage = Stage::from_level(level).unwrap();
            match resolution.active_train() {
                None => {
                    let base = resolution.stable_or_zero();
                    let target = match (bootstrap, for_target) {
                        (_, Some(ft)) => ft.into(),
                        (true, None) => BumpTarget::Minor,
                        (false, None) => BumpTarget::Patch,
                    };
                    let mut next = bump(&base, target);
                    next.pre = Prerelease::new(&format!("{}.1", stage.name()))?;
                    Ok(next)
                }
                Some(train) => {
                    let train_stage = Stage::from_prerelease(train)?;

                    if let Some(ft) = for_target {
                        let base = resolution.stable_or_zero();
                        let candidate = bump(&base, ft.into());
                        if (candidate.major, candidate.minor, candidate.patch)
                            != (train.major, train.minor, train.patch)
                        {
                            bail!(
                                "--for {ft:?} does not match the active train's target \
                                 ({train}); omit --for or match the existing train"
                            );
                        }
                    }

                    if stage < train_stage {
                        bail!(
                            "cannot release '{}' on top of an active '{}' train ({train}) — \
                             pre-release maturity only moves forward",
                            stage.name(),
                            train_stage.name()
                        );
                    }

                    let mut next = train.clone();
                    next.build = BuildMetadata::EMPTY;
                    if stage == train_stage {
                        let counter = current_counter(train)? + 1;
                        next.pre = Prerelease::new(&format!("{}.{}", stage.name(), counter))?;
                    } else {
                        next.pre = Prerelease::new(&format!("{}.1", stage.name()))?;
                    }
                    Ok(next)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::version::resolve;

    fn resolution(tags: &[&str]) -> Resolution {
        let tags: Vec<String> = tags.iter().map(|s| s.to_string()).collect();
        resolve(&tags, r"^v\d+\.\d+\.\d+").unwrap()
    }

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn patch_minor_major_bump_from_latest_stable() {
        let r = resolution(&["v1.4.2"]);
        assert_eq!(next_version(&r, Level::Patch, None).unwrap(), v("1.4.3"));
        assert_eq!(next_version(&r, Level::Minor, None).unwrap(), v("1.5.0"));
        assert_eq!(next_version(&r, Level::Major, None).unwrap(), v("2.0.0"));
    }

    #[test]
    fn no_train_rc_bumps_patch_by_default() {
        let r = resolution(&["v1.4.2"]);
        assert_eq!(next_version(&r, Level::Rc, None).unwrap(), v("1.4.3-rc.1"));
    }

    #[test]
    fn no_train_rc_for_minor() {
        let r = resolution(&["v1.4.2"]);
        assert_eq!(
            next_version(&r, Level::Rc, Some(ForTarget::Minor)).unwrap(),
            v("1.5.0-rc.1")
        );
    }

    #[test]
    fn active_train_same_stage_increments() {
        let r = resolution(&["v1.4.2", "v1.5.0-rc.2"]);
        assert_eq!(next_version(&r, Level::Rc, None).unwrap(), v("1.5.0-rc.3"));
    }

    #[test]
    fn active_train_new_higher_stage_resets_counter() {
        let r = resolution(&["v1.4.2", "v1.5.0-alpha.3"]);
        assert_eq!(
            next_version(&r, Level::Beta, None).unwrap(),
            v("1.5.0-beta.1")
        );
    }

    #[test]
    fn precedence_guard_rejects_backward_stage() {
        let r = resolution(&["v1.4.2", "v1.5.0-beta.2"]);
        let err = next_version(&r, Level::Alpha, None).unwrap_err();
        assert!(err
            .to_string()
            .contains("pre-release maturity only moves forward"));
    }

    #[test]
    fn stable_strips_prerelease_suffix() {
        let r = resolution(&["v1.4.2", "v1.5.0-rc.4"]);
        assert_eq!(next_version(&r, Level::Stable, None).unwrap(), v("1.5.0"));
    }

    #[test]
    fn stable_errors_without_active_train() {
        let r = resolution(&["v1.4.2"]);
        let err = next_version(&r, Level::Stable, None).unwrap_err();
        assert!(err.to_string().contains("no active pre-release train"));
    }

    #[test]
    fn bootstrap_always_targets_minor() {
        let r = resolution(&[]);
        assert_eq!(next_version(&r, Level::Patch, None).unwrap(), v("0.1.0"));
        assert_eq!(next_version(&r, Level::Minor, None).unwrap(), v("0.1.0"));
        assert_eq!(next_version(&r, Level::Major, None).unwrap(), v("0.1.0"));
        assert_eq!(next_version(&r, Level::Rc, None).unwrap(), v("0.1.0-rc.1"));
    }

    #[test]
    fn bootstrap_for_overrides_default() {
        let r = resolution(&[]);
        assert_eq!(
            next_version(&r, Level::Rc, Some(ForTarget::Major)).unwrap(),
            v("1.0.0-rc.1")
        );
    }

    #[test]
    fn for_flag_rejected_on_direct_levels() {
        let r = resolution(&["v1.4.2"]);
        assert!(next_version(&r, Level::Patch, Some(ForTarget::Minor)).is_err());
        assert!(next_version(&r, Level::Stable, Some(ForTarget::Minor)).is_err());
    }

    #[test]
    fn for_flag_mismatch_on_active_train_errors() {
        let r = resolution(&["v1.4.2", "v1.5.0-rc.1"]);
        let err = next_version(&r, Level::Rc, Some(ForTarget::Major)).unwrap_err();
        assert!(err
            .to_string()
            .contains("does not match the active train's target"));
    }

    #[test]
    fn for_flag_matching_active_train_is_accepted() {
        let r = resolution(&["v1.4.2", "v1.5.0-rc.1"]);
        assert_eq!(
            next_version(&r, Level::Rc, Some(ForTarget::Minor)).unwrap(),
            v("1.5.0-rc.2")
        );
    }

    #[test]
    fn orphaned_prerelease_train_is_reported_as_active() {
        // Documented git-hygiene edge case: no override machinery, it just
        // behaves like any other active train until the tag is deleted.
        let r = resolution(&["v1.4.2", "v1.5.0-rc.1"]);
        assert!(r.active_train().is_some());
        assert_eq!(next_version(&r, Level::Rc, None).unwrap(), v("1.5.0-rc.2"));
    }

    #[test]
    fn prerelease_only_history_still_bootstraps_to_minor() {
        // Regression: with only `v0.1.0-rc.1`, `patch` used to yield 0.0.1
        // (below the train) and `major` 1.0.0.
        let r = resolution(&["v0.1.0-rc.1"]);
        assert_eq!(next_version(&r, Level::Patch, None).unwrap(), v("0.1.0"));
        assert_eq!(next_version(&r, Level::Minor, None).unwrap(), v("0.1.0"));
        assert_eq!(next_version(&r, Level::Major, None).unwrap(), v("0.1.0"));
        assert_eq!(next_version(&r, Level::Rc, None).unwrap(), v("0.1.0-rc.2"));
    }

    #[test]
    fn prerelease_only_history_refuses_to_undercut_a_higher_train() {
        let r = resolution(&["v1.0.0-rc.1"]);
        let err = next_version(&r, Level::Patch, None).unwrap_err();
        assert!(err.to_string().contains("oxr release stable"), "{err}");
        assert_eq!(next_version(&r, Level::Stable, None).unwrap(), v("1.0.0"));
    }

    #[test]
    fn advancing_a_train_drops_build_metadata() {
        let r = resolution(&["v1.4.2", "v1.5.0-rc.2+build.7"]);
        // Version equality includes build metadata, so this fails if `+build.7` leaks.
        assert_eq!(next_version(&r, Level::Rc, None).unwrap(), v("1.5.0-rc.3"));
    }
}

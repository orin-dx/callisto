use callisto_model::{Severity, Version, VersionGrammar};

/// Trait for version arithmetic per versioning grammar.
pub trait Versioning: Send + Sync {
    fn grammar(&self) -> VersionGrammar;
    fn bump(&self, current: &Version, severity: Severity) -> Result<Version, BumpError>;
    fn bump_prerelease(
        &self,
        base: &Version,
        severity: Severity,
        tag: &str,
        current: &Version,
    ) -> Result<Version, BumpError>;
}

/// SemVer versioning arithmetic implementation matching @changesets/cli semantics.
pub struct SemVerVersioning;

static SEMVER_VERSIONING: SemVerVersioning = SemVerVersioning;

pub fn versioning_for(grammar: VersionGrammar) -> Option<&'static dyn Versioning> {
    match grammar {
        VersionGrammar::SemVer => Some(&SEMVER_VERSIONING),
        _ => None,
    }
}

impl Versioning for SemVerVersioning {
    fn grammar(&self) -> VersionGrammar {
        VersionGrammar::SemVer
    }

    fn bump(&self, current: &Version, severity: Severity) -> Result<Version, BumpError> {
        if current.grammar() != VersionGrammar::SemVer {
            return Err(BumpError::NotSemVer {
                raw: current.render().to_string(),
                grammar: current.grammar(),
            });
        }

        if severity == Severity::None {
            return Ok(current.clone());
        }

        let major = current.major().unwrap_or(0);
        let minor = current.minor().unwrap_or(0);
        let patch = current.patch().unwrap_or(0);

        if current.is_prerelease() {
            let base = Version::semver(major, minor, patch);
            let bumped = match severity {
                Severity::Patch => base,
                Severity::Minor => {
                    if patch == 0 {
                        base
                    } else {
                        Version::semver(major, minor + 1, 0)
                    }
                }
                Severity::Major => {
                    if minor == 0 && patch == 0 {
                        base
                    } else {
                        Version::semver(major + 1, 0, 0)
                    }
                }
                Severity::None => current.clone(),
            };
            return Ok(bumped);
        }

        let bumped = match severity {
            Severity::Major => Version::semver(major + 1, 0, 0),
            Severity::Minor => Version::semver(major, minor + 1, 0),
            Severity::Patch => Version::semver(major, minor, patch + 1),
            Severity::None => current.clone(),
        };

        Ok(bumped)
    }

    fn bump_prerelease(
        &self,
        base: &Version,
        severity: Severity,
        tag: &str,
        current: &Version,
    ) -> Result<Version, BumpError> {
        if severity == Severity::None {
            return Ok(current.clone());
        }

        let release = self.bump(base, severity)?;

        let mut counter = 0;
        if current.is_prerelease() {
            let current_raw = current.render();
            if let Some((rel_part, pre_part)) = current_raw.split_once('-') {
                if rel_part == release.render() {
                    let expected_prefix = format!("{tag}.");
                    if let Some(num_str) = pre_part.strip_prefix(&expected_prefix) {
                        if let Ok(num) = num_str.parse::<u64>() {
                            counter = num + 1;
                        }
                    }
                }
            }
        }

        let prerelease_str = format!("{}-{tag}.{counter}", release.render());
        let final_version =
            Version::parse(&prerelease_str, VersionGrammar::SemVer).map_err(|_| {
                BumpError::NotSemVer {
                    raw: prerelease_str,
                    grammar: VersionGrammar::SemVer,
                }
            })?;

        Ok(final_version)
    }
}

pub fn bump_version(current: &Version, severity: Severity) -> Result<Version, BumpError> {
    SEMVER_VERSIONING.bump(current, severity)
}

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum BumpError {
    #[error("bump_version requires a SemVer version; `{raw}` was parsed as {grammar:?}")]
    NotSemVer {
        raw: String,
        grammar: VersionGrammar,
    },
    #[error("no versioning implementation exists for {grammar:?}")]
    UnsupportedGrammar { grammar: VersionGrammar },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bumps_semver_versions() {
        let v = Version::parse("1.2.3", VersionGrammar::SemVer).unwrap();
        assert_eq!(bump_version(&v, Severity::Patch).unwrap().render(), "1.2.4");
        assert_eq!(bump_version(&v, Severity::Minor).unwrap().render(), "1.3.0");
        assert_eq!(bump_version(&v, Severity::Major).unwrap().render(), "2.0.0");
        assert_eq!(bump_version(&v, Severity::None).unwrap().render(), "1.2.3");
    }

    #[test]
    fn test_prerelease_to_stable_bump() {
        let v = Version::parse("1.2.3-alpha.0", VersionGrammar::SemVer).unwrap();
        assert_eq!(bump_version(&v, Severity::Patch).unwrap().render(), "1.2.3");
        assert_eq!(bump_version(&v, Severity::Minor).unwrap().render(), "1.3.0");
        assert_eq!(bump_version(&v, Severity::Major).unwrap().render(), "2.0.0");

        let v2 = Version::parse("1.0.0-beta.1", VersionGrammar::SemVer).unwrap();
        assert_eq!(
            bump_version(&v2, Severity::Patch).unwrap().render(),
            "1.0.0"
        );
        assert_eq!(
            bump_version(&v2, Severity::Minor).unwrap().render(),
            "1.0.0"
        );
        assert_eq!(
            bump_version(&v2, Severity::Major).unwrap().render(),
            "1.0.0"
        );
    }

    #[test]
    fn zero_x_bump_major_becomes_one_zero_zero() {
        let v = Version::parse("0.5.2", VersionGrammar::SemVer).unwrap();
        assert_eq!(bump_version(&v, Severity::Major).unwrap().render(), "1.0.0");
    }

    #[test]
    fn bump_prerelease_monotonic_counter() {
        let base = Version::parse("1.1.0", VersionGrammar::SemVer).unwrap();
        let cur = Version::parse("1.1.0", VersionGrammar::SemVer).unwrap();
        let pre0 = SEMVER_VERSIONING
            .bump_prerelease(&base, Severity::Patch, "next", &cur)
            .unwrap();
        assert_eq!(pre0.render(), "1.1.1-next.0");

        let pre1 = SEMVER_VERSIONING
            .bump_prerelease(&base, Severity::Patch, "next", &pre0)
            .unwrap();
        assert_eq!(pre1.render(), "1.1.1-next.1");
    }
}

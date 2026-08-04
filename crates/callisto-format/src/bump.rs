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

/// PEP 440 versioning arithmetic implementation.
///
/// Bump semantics (deliberately chosen, see the test matrix below):
/// - The release segment is treated as `major.minor.patch`, matching the SemVer
///   implementation's shape; any 4th+ release segment is dropped by a bump.
/// - The epoch (`N!`) has no SemVer analogue. It carries through a bump unchanged.
/// - Pre-releases (`aN`/`bN`/`rcN`) and dev-releases (`.devN`) both sort *before*
///   the final release they're attached to (PEP 440: `devN < aN < bN < rcN < final`),
///   so a bump finalizes them in place exactly like the SemVer prerelease branch:
///   `Severity::Patch` just drops the tag, `Minor`/`Major` only advance the next
///   component up when the lower one is already zero.
/// - Post-releases (`.postN`) sort *after* the final release they're attached to
///   (PEP 440: `final < .postN`), so the base release is already "shipped": a bump
///   increments normally (as if there were no pre/dev/post suffix at all) and drops
///   the post segment, rather than continuing the post sequence.
/// - Local version labels (`+localsuffix`) are always dropped by a bump.
pub struct Pep440Versioning;

static PEP440_VERSIONING: Pep440Versioning = Pep440Versioning;

pub fn versioning_for(grammar: VersionGrammar) -> Option<&'static dyn Versioning> {
    match grammar {
        VersionGrammar::SemVer => Some(&SEMVER_VERSIONING),
        VersionGrammar::Pep440 => Some(&PEP440_VERSIONING),
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
                    let dotted_prefix = format!("{tag}.");
                    if let Some(num_str) = pre_part.strip_prefix(&dotted_prefix) {
                        // Dotted form: e.g. "alpha.3" with tag "alpha" → num_str = "3"
                        if let Ok(num) = num_str.parse::<u64>() {
                            counter = num + 1;
                        }
                    } else if let Some(num_str) = pre_part.strip_prefix(tag) {
                        // Undotted form: e.g. "alpha1" with tag "alpha" → num_str = "1"
                        if let Ok(num) = num_str.parse::<u64>() {
                            counter = num + 1;
                        }
                    }
                }
            }
        }

        let prerelease_str = format!("{}-{tag}.{counter}", release.render());
        let final_version =
            Version::parse(&prerelease_str, VersionGrammar::SemVer).map_err(|_err| {
                BumpError::NotSemVer {
                    raw: prerelease_str,
                    grammar: VersionGrammar::SemVer,
                }
            })?;

        Ok(final_version)
    }
}

impl Versioning for Pep440Versioning {
    fn grammar(&self) -> VersionGrammar {
        VersionGrammar::Pep440
    }

    fn bump(&self, current: &Version, severity: Severity) -> Result<Version, BumpError> {
        if current.grammar() != VersionGrammar::Pep440 {
            return Err(BumpError::NotPep440 {
                raw: current.render().to_string(),
                grammar: current.grammar(),
            });
        }

        if severity == Severity::None {
            return Ok(current.clone());
        }

        let parsed = parse_pep440(current)?;
        let epoch = parsed.epoch();
        let (major, minor, patch) = release_triple(&parsed);

        // Pre-releases and dev-releases both precede the release they're attached
        // to, so a bump finalizes them in place rather than continuing forward.
        // But a post-release segment (even combined with a dev segment, e.g.
        // `1.2.3.post1.dev1`) outranks the final release (PEP 440 ordering:
        // dev < pre < release < post), so any version carrying a post segment
        // must bump forward normally, same as the post-only case, never
        // finalize in place.
        let finalize_in_place = !parsed.is_post() && (parsed.is_pre() || parsed.is_dev());

        let (new_major, new_minor, new_patch) = if finalize_in_place {
            match severity {
                Severity::Patch => (major, minor, patch),
                Severity::Minor => {
                    if patch == 0 {
                        (major, minor, patch)
                    } else {
                        (major, minor + 1, 0)
                    }
                }
                Severity::Major => {
                    if minor == 0 && patch == 0 {
                        (major, minor, patch)
                    } else {
                        (major + 1, 0, 0)
                    }
                }
                Severity::None => unreachable!("handled above"),
            }
        } else {
            match severity {
                Severity::Major => (major + 1, 0, 0),
                Severity::Minor => (major, minor + 1, 0),
                Severity::Patch => (major, minor, patch + 1),
                Severity::None => unreachable!("handled above"),
            }
        };

        let bumped = pep440_rs::Version::new([new_major, new_minor, new_patch]).with_epoch(epoch);
        render_pep440(bumped)
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
        let release_str = release.render();

        // PEP 440 pre-release labels are restricted to alpha/beta/rc; an arbitrary
        // caller-supplied tag (e.g. "next") has no PEP 440 equivalent, so it falls
        // back to a dev-release, the closest "not yet final" concept PEP 440 has.
        let letter = pep440_prerelease_letter(tag);

        let mut counter = 0u64;
        let current_raw = current.render();
        if let Some(rest) = current_raw.strip_prefix(release_str) {
            let num_str = match letter {
                Some(letter) => rest.strip_prefix(letter),
                None => rest.strip_prefix(".dev"),
            };
            if let Some(num_str) = num_str {
                if let Ok(num) = num_str.parse::<u64>() {
                    counter = num + 1;
                }
            }
        }

        let prerelease_str = match letter {
            Some(letter) => format!("{release_str}{letter}{counter}"),
            None => format!("{release_str}.dev{counter}"),
        };

        Version::parse(&prerelease_str, VersionGrammar::Pep440).map_err(|e| {
            BumpError::ComputedVersionInvalid {
                raw: prerelease_str,
                message: e.message,
            }
        })
    }
}

/// Maps common pre-release tag spellings to their PEP 440 letter. PEP 440 only
/// recognizes alpha/beta/rc pre-releases; anything else has no direct mapping.
fn pep440_prerelease_letter(tag: &str) -> Option<&'static str> {
    match tag.to_ascii_lowercase().as_str() {
        "a" | "alpha" => Some("a"),
        "b" | "beta" => Some("b"),
        "rc" | "c" | "pre" | "preview" => Some("rc"),
        _ => None,
    }
}

fn parse_pep440(v: &Version) -> Result<pep440_rs::Version, BumpError> {
    v.raw()
        .parse::<pep440_rs::Version>()
        .map_err(|e| BumpError::ComputedVersionInvalid {
            raw: v.raw().to_string(),
            message: e.to_string(),
        })
}

fn release_triple(v: &pep440_rs::Version) -> (u64, u64, u64) {
    let release = v.release();
    (
        release.first().copied().unwrap_or(0),
        release.get(1).copied().unwrap_or(0),
        release.get(2).copied().unwrap_or(0),
    )
}

fn render_pep440(v: pep440_rs::Version) -> Result<Version, BumpError> {
    let rendered = v.to_string();
    Version::parse(&rendered, VersionGrammar::Pep440).map_err(|e| {
        BumpError::ComputedVersionInvalid {
            raw: rendered,
            message: e.message,
        }
    })
}

pub fn bump_version(current: &Version, severity: Severity) -> Result<Version, BumpError> {
    SEMVER_VERSIONING.bump(current, severity)
}

#[derive(Clone, Debug, thiserror::Error, miette::Diagnostic, PartialEq, Eq)]
#[non_exhaustive]
pub enum BumpError {
    #[error("bump_version requires a SemVer version; `{raw}` was parsed as {grammar:?}")]
    #[diagnostic(code(E035))]
    NotSemVer {
        raw: String,
        grammar: VersionGrammar,
    },
    #[error("no versioning implementation exists for {grammar:?}")]
    #[diagnostic(code(E036))]
    UnsupportedGrammar { grammar: VersionGrammar },
    #[error("bump requires a PEP 440 version; `{raw}` was parsed as {grammar:?}")]
    #[diagnostic(code(E037))]
    NotPep440 {
        raw: String,
        grammar: VersionGrammar,
    },
    #[error("internal error computing bumped version `{raw}`: {message}")]
    #[diagnostic(code(E038))]
    ComputedVersionInvalid { raw: String, message: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case("1.2.3", Severity::Patch, "1.2.4")]
    #[case("1.2.3", Severity::Minor, "1.3.0")]
    #[case("1.2.3", Severity::Major, "2.0.0")]
    #[case("1.2.3", Severity::None, "1.2.3")]
    #[case("1.2.3-alpha.0", Severity::Patch, "1.2.3")]
    #[case("1.2.3-alpha.0", Severity::Minor, "1.3.0")]
    #[case("1.2.3-alpha.0", Severity::Major, "2.0.0")]
    #[case("1.0.0-beta.1", Severity::Patch, "1.0.0")]
    #[case("1.0.0-beta.1", Severity::Minor, "1.0.0")]
    #[case("1.0.0-beta.1", Severity::Major, "1.0.0")]
    #[case("0.5.2", Severity::Major, "1.0.0")]
    fn test_semver_bump_matrix(#[case] input: &str, #[case] sev: Severity, #[case] expected: &str) {
        let v = Version::parse(input, VersionGrammar::SemVer).unwrap();
        assert_eq!(bump_version(&v, sev).unwrap().render(), expected);
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

    #[test]
    fn versioning_for_pep440_returns_an_impl() {
        assert!(versioning_for(VersionGrammar::Pep440).is_some());
    }

    #[rstest]
    // Plain releases: ordinary major/minor/patch arithmetic, same as SemVer.
    #[case("1.2.3", Severity::Patch, "1.2.4")]
    #[case("1.2.3", Severity::Minor, "1.3.0")]
    #[case("1.2.3", Severity::Major, "2.0.0")]
    #[case("1.2.3", Severity::None, "1.2.3")]
    #[case("0.5.2", Severity::Major, "1.0.0")]
    // Pre-releases (aN/bN/rcN) finalize toward the base release, mirroring the
    // SemVer prerelease branch: a patch bump just drops the prerelease tag,
    // minor/major only increment when the lower component is non-zero.
    #[case("1.2.3a1", Severity::Patch, "1.2.3")]
    #[case("1.2.3a1", Severity::Minor, "1.3.0")]
    #[case("1.2.3b2", Severity::Major, "2.0.0")]
    #[case("1.0.0rc1", Severity::Minor, "1.0.0")]
    #[case("1.0.0rc1", Severity::Major, "1.0.0")]
    // Dev-releases (.devN) precede the final release (PEP 440 ordering:
    // devN < aN < bN < rcN < final), so they finalize the same way pre-releases do.
    #[case("1.2.3.dev0", Severity::Patch, "1.2.3")]
    #[case("1.2.3.dev0", Severity::Minor, "1.3.0")]
    // Post-releases (.postN) come *after* an already-shipped final release
    // (PEP 440 ordering: final < .postN), so a bump treats the release as
    // already final: it increments normally and drops the post segment,
    // rather than finalizing in place like a pre/dev release would.
    #[case("1.2.3.post1", Severity::Patch, "1.2.4")]
    #[case("1.2.3.post1", Severity::Minor, "1.3.0")]
    // A post-release combined with a dev-release (e.g. `.post1.dev1`) still
    // carries a post segment, which PEP 440 ranks *above* the final release
    // even with a trailing dev segment attached (dev < pre < release < post).
    // It must bump forward exactly like the post-only case above, never
    // finalize in place (which would otherwise produce a version lower than
    // the input).
    #[case("1.2.3.post1.dev1", Severity::Patch, "1.2.4")]
    // Epoch (N!) has no SemVer analogue and carries through unchanged.
    #[case("1!2.0.0", Severity::Major, "1!3.0.0")]
    #[case("1!2.0.0", Severity::Patch, "1!2.0.1")]
    fn test_pep440_bump_matrix(#[case] input: &str, #[case] sev: Severity, #[case] expected: &str) {
        let v = Version::parse(input, VersionGrammar::Pep440).unwrap();
        let versioning =
            versioning_for(VersionGrammar::Pep440).expect("pep440 versioning should be registered");
        assert_eq!(versioning.bump(&v, sev).unwrap().render(), expected);
    }

    #[test]
    fn pep440_bump_rejects_non_pep440_grammar() {
        let v = Version::parse("1.2.3", VersionGrammar::SemVer).unwrap();
        let versioning = versioning_for(VersionGrammar::Pep440).unwrap();
        let err = versioning.bump(&v, Severity::Patch).unwrap_err();
        assert!(matches!(err, BumpError::NotPep440 { .. }));
    }

    #[test]
    fn pep440_bump_prerelease_uses_recognized_letter_and_is_monotonic() {
        let versioning = versioning_for(VersionGrammar::Pep440).unwrap();
        let base = Version::parse("1.1.0", VersionGrammar::Pep440).unwrap();
        let cur = Version::parse("1.1.0", VersionGrammar::Pep440).unwrap();

        let pre0 = versioning
            .bump_prerelease(&base, Severity::Patch, "rc", &cur)
            .unwrap();
        assert_eq!(pre0.render(), "1.1.1rc0");

        let pre1 = versioning
            .bump_prerelease(&base, Severity::Patch, "rc", &pre0)
            .unwrap();
        assert_eq!(pre1.render(), "1.1.1rc1");
    }

    #[test]
    fn pep440_bump_prerelease_falls_back_to_dev_for_unrecognized_tag() {
        let versioning = versioning_for(VersionGrammar::Pep440).unwrap();
        let base = Version::parse("1.1.0", VersionGrammar::Pep440).unwrap();
        let cur = Version::parse("1.1.0", VersionGrammar::Pep440).unwrap();

        let pre0 = versioning
            .bump_prerelease(&base, Severity::Patch, "next", &cur)
            .unwrap();
        assert_eq!(pre0.render(), "1.1.1.dev0");

        let pre1 = versioning
            .bump_prerelease(&base, Severity::Patch, "next", &pre0)
            .unwrap();
        assert_eq!(pre1.render(), "1.1.1.dev1");
    }

    /// Gap 1: epoch + post + dev combined in a single version. The epoch-carry
    /// logic and the "post outranks dev/pre, always bump forward" branch have
    /// only ever been exercised separately (see `1!2.0.0` and `.post1.dev1`
    /// cases above); this exercises both simultaneously. Observed/expected:
    /// the epoch is preserved unchanged and the release bumps forward exactly
    /// as the post-only case does (post always wins over the attached dev tag).
    #[rstest]
    #[case("1!2.3.post1.dev1", Severity::Patch, "1!2.3.1")]
    #[case("1!2.3.post1.dev1", Severity::Minor, "1!2.4.0")]
    #[case("1!2.3.post1.dev1", Severity::Major, "1!3.0.0")]
    fn test_pep440_bump_epoch_post_dev_combined(
        #[case] input: &str,
        #[case] sev: Severity,
        #[case] expected: &str,
    ) {
        let v = Version::parse(input, VersionGrammar::Pep440).unwrap();
        let versioning = versioning_for(VersionGrammar::Pep440).unwrap();
        assert_eq!(versioning.bump(&v, sev).unwrap().render(), expected);
    }

    /// Gap 2: a PEP 440 local version identifier (`+build.5`). Verified
    /// empirically (not assumed from the doc comment): the local segment is
    /// dropped cleanly by a bump, and the release arithmetic proceeds
    /// normally on the release triple, matching the module doc comment's
    /// claim that local labels are "always dropped by a bump".
    #[rstest]
    #[case("1.2.3+build.5", Severity::Patch, "1.2.4")]
    #[case("1.2.3+build.5", Severity::Minor, "1.3.0")]
    #[case("1.2.3+build.5", Severity::Major, "2.0.0")]
    fn test_pep440_bump_drops_local_version_label(
        #[case] input: &str,
        #[case] sev: Severity,
        #[case] expected: &str,
    ) {
        let v = Version::parse(input, VersionGrammar::Pep440).unwrap();
        let versioning = versioning_for(VersionGrammar::Pep440).unwrap();
        assert_eq!(versioning.bump(&v, sev).unwrap().render(), expected);
    }

    /// Gap 3: all-zero `0.0.0` base version bumped by Minor/Major. Plain
    /// (non-prerelease) releases go through the ordinary forward-bump branch,
    /// which has no zero-guard and behaves like any other release. The
    /// `patch == 0` / `minor == 0 && patch == 0` boundary guards only exist in
    /// the prerelease-finalization branch, so they are exercised here with an
    /// all-zero *prerelease* base (`0.0.0-alpha.0` / `0.0.0a1`), where the
    /// guard causes the bump to collapse back to the unchanged base `0.0.0`
    /// instead of advancing past it.
    #[test]
    fn semver_zero_base_plain_bump() {
        let v = Version::parse("0.0.0", VersionGrammar::SemVer).unwrap();
        assert_eq!(bump_version(&v, Severity::Minor).unwrap().render(), "0.1.0");
        let v = Version::parse("0.0.0", VersionGrammar::SemVer).unwrap();
        assert_eq!(bump_version(&v, Severity::Major).unwrap().render(), "1.0.0");
    }

    #[test]
    fn semver_zero_base_prerelease_bump_hits_zero_guards() {
        let v = Version::parse("0.0.0-alpha.0", VersionGrammar::SemVer).unwrap();
        assert_eq!(bump_version(&v, Severity::Minor).unwrap().render(), "0.0.0");
        let v = Version::parse("0.0.0-alpha.0", VersionGrammar::SemVer).unwrap();
        assert_eq!(bump_version(&v, Severity::Major).unwrap().render(), "0.0.0");
    }

    #[test]
    fn pep440_zero_base_plain_bump() {
        let versioning = versioning_for(VersionGrammar::Pep440).unwrap();
        let v = Version::parse("0.0.0", VersionGrammar::Pep440).unwrap();
        assert_eq!(
            versioning.bump(&v, Severity::Minor).unwrap().render(),
            "0.1.0"
        );
        let v = Version::parse("0.0.0", VersionGrammar::Pep440).unwrap();
        assert_eq!(
            versioning.bump(&v, Severity::Major).unwrap().render(),
            "1.0.0"
        );
    }

    #[test]
    fn pep440_zero_base_prerelease_bump_hits_zero_guards() {
        let versioning = versioning_for(VersionGrammar::Pep440).unwrap();
        let v = Version::parse("0.0.0a1", VersionGrammar::Pep440).unwrap();
        assert_eq!(
            versioning.bump(&v, Severity::Minor).unwrap().render(),
            "0.0.0"
        );
        let v = Version::parse("0.0.0a1", VersionGrammar::Pep440).unwrap();
        assert_eq!(
            versioning.bump(&v, Severity::Major).unwrap().render(),
            "0.0.0"
        );
    }

    /// Gap 4: a genuinely malformed PEP 440 string passed to the public parse
    /// entry point returns a proper `Err`, never panics.
    /// Regression test: an undotted prerelease suffix such as `alpha1` (common in
    /// packages migrated from other toolchains) must have its numeric suffix
    /// carried forward rather than reset to 0.  Before the fix, the only prefix
    /// pattern tested was the dotted form `"{tag}."`, so `strip_prefix("alpha.")`
    /// on `"alpha1"` returned `None`, the counter stayed at 0, and the output was
    /// `1.2.3-alpha.0` — meaning consecutive bumps would silently lose the
    /// existing sequence number.
    ///
    /// Note on SemVer ordering: `"alpha"` < `"alpha1"` lexicographically (the
    /// dotted identifier `alpha` is a strict ASCII prefix of `alpha1`), so
    /// `1.2.3-alpha.2 < 1.2.3-alpha1` per spec.  The invariant we can assert is
    /// that the counter increments from the existing numeric suffix (not from 0),
    /// so future same-format bumps remain monotonically increasing.
    #[test]
    fn bump_prerelease_from_undotted_tag_produces_higher_version() {
        // Use base="1.2.2" + Patch so that bump(base, Patch)="1.2.3", matching
        // the release segment of the current undotted prerelease versions below.
        let base = Version::parse("1.2.2", VersionGrammar::SemVer).unwrap();
        let buggy_base = Version::parse("1.2.3-alpha.0", VersionGrammar::SemVer).unwrap();

        // 1.2.3-alpha1 → 1.2.3-alpha.2  (numeric suffix 1 → counter = 2)
        let cur = Version::parse("1.2.3-alpha1", VersionGrammar::SemVer).unwrap();
        let result = SEMVER_VERSIONING
            .bump_prerelease(&base, Severity::Patch, "alpha", &cur)
            .unwrap();
        assert_eq!(result.render(), "1.2.3-alpha.2");
        // The result must be greater than the buggy alpha.0 output that the
        // pre-fix code would have produced, ensuring the sequence moves forward.
        assert_eq!(
            result.compare(&buggy_base).unwrap(),
            std::cmp::Ordering::Greater,
            "1.2.3-alpha.2 must be SemVer-greater than the buggy 1.2.3-alpha.0"
        );

        // 1.2.3-alpha9 → 1.2.3-alpha.10  (no off-by-one at boundary 9 → 10)
        let cur9 = Version::parse("1.2.3-alpha9", VersionGrammar::SemVer).unwrap();
        let result9 = SEMVER_VERSIONING
            .bump_prerelease(&base, Severity::Patch, "alpha", &cur9)
            .unwrap();
        assert_eq!(result9.render(), "1.2.3-alpha.10");
        assert_eq!(
            result9.compare(&buggy_base).unwrap(),
            std::cmp::Ordering::Greater,
            "1.2.3-alpha.10 must be SemVer-greater than the buggy 1.2.3-alpha.0"
        );
    }

    #[test]
    fn pep440_parse_malformed_string_returns_err_not_panic() {
        let result = Version::parse("not-a-version", VersionGrammar::Pep440);
        assert!(result.is_err());
    }

    /// Gap 5: `Severity::None` applied to a post-tagged or dev-tagged input
    /// leaves the version completely unchanged (early-return before any
    /// finalize/bump-forward branch runs), same as it does for a plain
    /// release.
    #[rstest]
    #[case("1.2.3.post1")]
    #[case("1.2.3.dev0")]
    #[case("1!2.3.post1.dev1")]
    fn pep440_severity_none_leaves_post_and_dev_tagged_input_unchanged(#[case] input: &str) {
        let v = Version::parse(input, VersionGrammar::Pep440).unwrap();
        let versioning = versioning_for(VersionGrammar::Pep440).unwrap();
        assert_eq!(
            versioning.bump(&v, Severity::None).unwrap().render(),
            v.render()
        );
    }
}

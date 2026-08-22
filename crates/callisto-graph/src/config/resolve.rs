use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use callisto_model::{
    ConfigKey, Ecosystem, PackageId, PublishTarget, RegistryKey, ReleaseTrigger, Severity, TagTemplate,
};

use crate::config::groups::{GroupTable, RawGroupTable};
use crate::config::pattern::PackagePattern;
use crate::config::raw::RawConfig;
use crate::error::{ConfigError, GraphError};

#[derive(Clone, Debug)]
pub struct ResolvedConfig {
    pub root: PathBuf,
    pub changesets_dir: PathBuf,
    pub cascade: CascadeConfig,
    pub validation: ValidationConfig,
    pub registries: BTreeMap<RegistryKey, RegistryConfig>,
    /// Per-package override rules from `[[package]]` blocks, in TOML declaration order.
    ///
    /// Lookup uses two-pass specificity (see `resolve_package_config`):
    /// - Pass 1 (Prefixed tier): the first entry whose `rule_id.ecosystem().is_some()` AND
    ///   `rule_id.matches(pkg_id)` is true wins, regardless of its position relative to Bare rules.
    /// - Pass 2 (Bare tier): only if Pass 1 finds nothing, the first entry where
    ///   `rule_id.matches(pkg_id)` is true wins.
    ///   Within each tier, first-match-wins in TOML declaration order.
    pub packages: Vec<(PackageId, PackageConfig)>,
    /// Bulk config-override rules from `[[package-set]]` blocks, in TOML declaration order.
    /// Applied as a fallback when no `[[package]]` rule matches a package.
    /// Unlike `[[package]]` (exact `PackageId` match, first-wins), each
    /// `[[package-set]]` rule uses a glob pattern and can match many packages simultaneously.
    pub package_sets: Vec<(PackagePattern, PackageConfig)>,
    pub groups: GroupTable,
    /// Raw group declarations from `callisto.toml`, kept so that
    /// `Workspace::load` can call `GroupTable::resolve` once the
    /// `IdentityIndex` is available after `ManifestWalkResolver::build`.
    pub(crate) raw_groups: RawGroupTable,
    pub promoted_siblings: BTreeMap<String, Vec<(PackageId, BTreeSet<Ecosystem>)>>,
    provenance: BTreeMap<ConfigKey, ConfigProvenance>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigProvenance {
    Default,
    Explicit,
}

impl ResolvedConfig {
    pub(crate) fn with_promoted_siblings(
        &self,
        promoted_siblings: BTreeMap<String, Vec<(PackageId, BTreeSet<Ecosystem>)>>,
    ) -> ResolvedConfig {
        let mut overlay = self.clone();
        overlay.promoted_siblings = promoted_siblings;
        overlay
    }

    pub fn provenance(&self, key: &ConfigKey) -> ConfigProvenance {
        self.provenance.get(key).copied().unwrap_or(ConfigProvenance::Default)
    }

    pub fn rendered_value(&self, key: &ConfigKey) -> Option<String> {
        if key == &ConfigKey::CASCADE_MODE {
            Some(match self.cascade.mode {
                CascadeMode::OutOfRange => "out-of-range".to_string(),
                CascadeMode::Always => "always".to_string(),
            })
        } else if key == &ConfigKey::CASCADE_BUMP_SEVERITY {
            Some(match self.cascade.bump_severity {
                CascadeBumpSeverity::Patch => "patch".to_string(),
                CascadeBumpSeverity::Minor => "minor".to_string(),
            })
        } else if key == &ConfigKey::CASCADE_PEER_ESCALATION {
            Some(self.cascade.peer_escalation.to_string())
        } else if key == &ConfigKey::CASCADE_PRESERVE_NPM_RANGES {
            Some(self.cascade.preserve_npm_ranges.to_string())
        } else if key == &ConfigKey::VALIDATION_ALLOW_EMPTY_CHANGESETS {
            Some(self.validation.allow_empty_changesets.to_string())
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CascadeConfig {
    pub mode: CascadeMode,
    pub bump_severity: CascadeBumpSeverity,
    pub peer_escalation: bool,
    pub preserve_npm_ranges: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CascadeMode {
    OutOfRange,
    Always,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CascadeBumpSeverity {
    Patch,
    Minor,
}

impl CascadeBumpSeverity {
    pub fn as_severity(self) -> Severity {
        match self {
            CascadeBumpSeverity::Patch => Severity::Patch,
            CascadeBumpSeverity::Minor => Severity::Minor,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ValidationConfig {
    pub allow_empty_changesets: bool,
}

#[derive(Clone, Debug)]
pub struct RegistryConfig {
    pub kind: Ecosystem,
    pub url: Option<String>,
}

/// Per-package overrides from a `[[package]]` block in `callisto.toml`.
///
/// Every field is `Option<T>` — `None` means "not specified; use the package's default."
/// Only fields that the user explicitly set in the `[[package]]` block are `Some`.
#[derive(Clone, Debug)]
pub struct PackageConfig {
    pub release_trigger: Option<ReleaseTrigger>,
    pub publish_to: Option<Vec<PublishTarget>>,
    pub tag_template: Option<TagTemplate>,
    /// Changelog path relative to the package's own root directory.
    pub changelog: Option<PathBuf>,
    pub pre_major_inference: Option<PreMajorInferencePolicy>,
}

/// Pre-1.0 (`0.y.z`) severity-downgrade policy. A closed 3-state choice --
/// modeled as an enum rather than two independent bools so the type system
/// rules out the unreachable-via-parser 4th combination a `{breaking_to_minor:
/// false, feat_to_patch: true}`-shaped struct literal could otherwise
/// construct.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PreMajorInferencePolicy {
    #[default]
    Off,
    /// Downgrades an inferred Major bump to Minor.
    Conservative,
    /// Downgrades an inferred Major bump to Minor, and Minor to Patch.
    ConservativeFeat,
}

pub fn parse_publish_target(s: &str) -> Result<PublishTarget, ConfigError> {
    match s {
        "crates-io" => Ok(PublishTarget::CratesIo),
        "npm" => Ok(PublishTarget::Npm {
            registry: None,
            access: None,
        }),
        "pypi" => Ok(PublishTarget::Pypi { index: None }),
        "nuget" => Ok(PublishTarget::NuGet { source: None }),
        "github-release" => Ok(PublishTarget::GitHubRelease),
        "none" => Ok(PublishTarget::None),
        other => Err(ConfigError::UnknownKey {
            path: PathBuf::new(),
            key: format!("publish-to = {other:?}"),
        }),
    }
}

pub fn parse_release_trigger(s: &str) -> Result<ReleaseTrigger, ConfigError> {
    match s {
        "changeset" => Ok(ReleaseTrigger::Changeset),
        "auto" => Ok(ReleaseTrigger::Auto),
        other => Err(ConfigError::UnknownKey {
            path: PathBuf::new(),
            key: format!("release-trigger = {other}"),
        }),
    }
}

/// Two-pass `[[package]]` rule lookup with Prefixed-over-Bare specificity.
///
/// Pass 1: iterate `cfg.packages` in Vec (TOML declaration) order. Return a reference
/// to the `PackageConfig` of the first entry `(rule_id, pkg_cfg)` where
/// `rule_id.ecosystem().is_some()` AND `rule_id.matches(id)`. Prefixed rules
/// (those with an explicit ecosystem prefix such as `cargo/`, `npm/`, `pypi/`)
/// always beat Bare rules regardless of declaration order in `callisto.toml`.
///
/// Pass 2 (only if Pass 1 found nothing): iterate `cfg.packages` in Vec order.
/// Return the first entry where `rule_id.matches(id)` (no ecosystem restriction).
///
/// Returns `None` if neither pass finds a match.
///
/// The function MUST NOT sort, partition-then-sort, or otherwise reorder `cfg.packages`.
/// Two-pass specificity is achieved solely by two separate linear scans over the
/// unmodified slice.
pub(crate) fn resolve_package_config<'a>(
    id: &PackageId,
    cfg: &'a ResolvedConfig,
) -> Result<Option<&'a PackageConfig>, GraphError> {
    if let Some((_, pcfg)) = cfg
        .packages
        .iter()
        .find(|(rule_id, _)| rule_id.ecosystem().is_some() && rule_id.matches(id))
    {
        return Ok(Some(pcfg));
    }
    if let Some((_, pcfg)) = cfg.packages.iter().find(|(rule_id, _)| rule_id.matches(id)) {
        if let Some(siblings) = cfg.promoted_siblings.get(id.name()) {
            return Err(GraphError::AmbiguousName {
                name: id.name().to_string(),
                candidates: siblings.iter().map(|(pid, _)| pid.clone()).collect(),
            });
        }
        return Ok(Some(pcfg));
    }
    Ok(None)
}

pub fn parse_pre_major_policy(s: &str) -> Result<PreMajorInferencePolicy, ConfigError> {
    match s {
        "off" | "false" => Ok(PreMajorInferencePolicy::Off),
        "conservative" => Ok(PreMajorInferencePolicy::Conservative),
        "conservative-feat" => Ok(PreMajorInferencePolicy::ConservativeFeat),
        _ => Err(ConfigError::InvalidPreMajorInference { found: s.to_string() }),
    }
}

pub fn load(root: &Path) -> Result<ResolvedConfig, ConfigError> {
    let callisto_toml = root.join("callisto.toml");
    let raw = if callisto_toml.exists() {
        let content = fs::read_to_string(&callisto_toml).map_err(|e| ConfigError::Read {
            path: callisto_toml.clone(),
            message: e.to_string(),
        })?;
        toml::from_str::<RawConfig>(&content).map_err(|e| ConfigError::ParseToml {
            path: callisto_toml.clone(),
            message: e.to_string(),
        })?
    } else {
        RawConfig::default()
    };

    let mut provenance = BTreeMap::new();

    let changesets_dir_str = raw
        .changesets
        .as_ref()
        .and_then(|c| c.dir.as_deref())
        .unwrap_or(".changeset");

    // Reject any changesets.dir value that is absolute or contains '..'
    // components — either would allow load_changesets / atomic_write to
    // escape the workspace root. We check components() rather than
    // canonicalizing because the directory may not exist yet (e.g. a fresh
    // workspace).
    let changesets_dir =
        callisto_model::workspace_relative(changesets_dir_str).map_err(|_err| ConfigError::InvalidChangesetsDir {
            dir: changesets_dir_str.to_string(),
        })?;

    let cascade_raw = raw.cascade.unwrap_or_default();
    let mode = match cascade_raw.mode.as_deref() {
        Some("always") => {
            provenance.insert(ConfigKey::CASCADE_MODE, ConfigProvenance::Explicit);
            CascadeMode::Always
        }
        Some("out-of-range") | None => CascadeMode::OutOfRange,
        Some(other) => {
            return Err(ConfigError::UnknownKey {
                path: callisto_toml,
                key: format!("cascade.mode = {other}"),
            })
        }
    };

    let bump_severity = match cascade_raw.bump_severity.as_deref() {
        Some("minor") => {
            provenance.insert(ConfigKey::CASCADE_BUMP_SEVERITY, ConfigProvenance::Explicit);
            CascadeBumpSeverity::Minor
        }
        Some("patch") | None => CascadeBumpSeverity::Patch,
        Some(other) => {
            return Err(ConfigError::InvalidBumpSeverity {
                found: other.to_string(),
            })
        }
    };

    let peer_escalation = cascade_raw.peer_escalation.unwrap_or(true);
    if cascade_raw.peer_escalation.is_some() {
        provenance.insert(ConfigKey::CASCADE_PEER_ESCALATION, ConfigProvenance::Explicit);
    }

    let preserve_npm_ranges = cascade_raw.preserve_npm_ranges.unwrap_or(true);
    if cascade_raw.preserve_npm_ranges.is_some() {
        provenance.insert(ConfigKey::CASCADE_PRESERVE_NPM_RANGES, ConfigProvenance::Explicit);
    }

    let validation_raw = raw.validation.unwrap_or_default();
    let allow_empty_changesets = validation_raw.allow_empty_changesets.unwrap_or(false);
    if validation_raw.allow_empty_changesets.is_some() {
        provenance.insert(ConfigKey::VALIDATION_ALLOW_EMPTY_CHANGESETS, ConfigProvenance::Explicit);
    }

    let mut registries = BTreeMap::new();
    registries.insert(
        RegistryKey(RegistryKey::CRATES_IO.to_string()),
        RegistryConfig {
            kind: Ecosystem::Cargo,
            url: None,
        },
    );
    registries.insert(
        RegistryKey(RegistryKey::NPM.to_string()),
        RegistryConfig {
            kind: Ecosystem::Npm,
            url: None,
        },
    );

    if let Some(raw_regs) = raw.registries {
        for (k_str, reg) in raw_regs {
            let key = RegistryKey(k_str);
            let kind = match reg.kind.as_deref() {
                Some("cargo") => Ecosystem::Cargo,
                Some("npm") | None => Ecosystem::Npm,
                Some("pypi") => Ecosystem::Pypi,
                Some(other) => {
                    return Err(ConfigError::UnknownKey {
                        path: callisto_toml.clone(),
                        key: format!("[registries] kind = {other:?}"),
                    })
                }
            };
            registries.insert(key, RegistryConfig { kind, url: reg.url });
        }
    }

    let raw_groups = RawGroupTable {
        fixed: raw.fixed_group.unwrap_or_default(),
        linked: raw.linked_group.unwrap_or_default(),
    };
    GroupTable::validate_syntactic(&raw_groups)?;

    // Resolve [[package]] blocks into per-package override rules.
    // Order is preserved: first matching rule wins during package construction.
    let mut packages: Vec<(PackageId, PackageConfig)> = Vec::new();
    for raw_pkg in raw.package.unwrap_or_default() {
        let pattern = PackageId::parse(&raw_pkg.pattern).map_err(|e| ConfigError::UnknownKey {
            path: callisto_toml.clone(),
            key: format!("[[package]] match = {:?}: {e}", raw_pkg.pattern),
        })?;

        let release_trigger = raw_pkg
            .release_trigger
            .as_deref()
            .map(parse_release_trigger)
            .transpose()?;

        let tag_template = raw_pkg
            .tag_template
            .as_deref()
            .map(TagTemplate::parse)
            .transpose()
            .map_err(ConfigError::Tag)?;

        let changelog = raw_pkg
            .changelog
            .as_deref()
            .map(|s| {
                callisto_model::workspace_relative(s).map_err(|_err| ConfigError::InvalidChangelogPath {
                    pattern: raw_pkg.pattern.clone(),
                    value: s.to_string(),
                })
            })
            .transpose()?;

        let pre_major_inference = raw_pkg
            .pre_major_inference
            .as_deref()
            .map(parse_pre_major_policy)
            .transpose()?;

        let publish_to = raw_pkg
            .publish_to
            .as_deref()
            .map(|targets| {
                targets
                    .iter()
                    .map(|s| parse_publish_target(s))
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()
            .map_err(|e| match e {
                ConfigError::UnknownKey { key, .. } => ConfigError::UnknownKey {
                    path: callisto_toml.clone(),
                    key: format!("[[package]] publish-to: {key}"),
                },
                other => other,
            })?;

        packages.push((
            pattern,
            PackageConfig {
                release_trigger,
                publish_to,
                tag_template,
                changelog,
                pre_major_inference,
            },
        ));
    }

    // Resolve [[package-set]] blocks into bulk config-override rules.
    // Semantics: each rule matches ALL packages whose PackageId matches the pattern.
    // A [[package]] rule takes priority; [[package-set]] is the fallback (see walk.rs).
    let mut package_sets: Vec<(PackagePattern, PackageConfig)> = Vec::new();
    for raw_pkg in raw.package_set.unwrap_or_default() {
        let pattern = PackagePattern::parse(&raw_pkg.pattern).map_err(|e| ConfigError::UnknownKey {
            path: callisto_toml.clone(),
            key: format!("[[package-set]] match = {:?}: {e}", raw_pkg.pattern),
        })?;

        let release_trigger = raw_pkg
            .release_trigger
            .as_deref()
            .map(parse_release_trigger)
            .transpose()?;

        let tag_template = raw_pkg
            .tag_template
            .as_deref()
            .map(TagTemplate::parse)
            .transpose()
            .map_err(ConfigError::Tag)?;

        let changelog = raw_pkg
            .changelog
            .as_deref()
            .map(|s| {
                callisto_model::workspace_relative(s).map_err(|_err| ConfigError::InvalidChangelogPath {
                    pattern: raw_pkg.pattern.clone(),
                    value: s.to_string(),
                })
            })
            .transpose()?;

        let pre_major_inference = raw_pkg
            .pre_major_inference
            .as_deref()
            .map(parse_pre_major_policy)
            .transpose()?;

        let publish_to = raw_pkg
            .publish_to
            .as_deref()
            .map(|targets| {
                targets
                    .iter()
                    .map(|s| parse_publish_target(s))
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()
            .map_err(|e| match e {
                ConfigError::UnknownKey { key, .. } => ConfigError::UnknownKey {
                    path: callisto_toml.clone(),
                    key: format!("[[package-set]] publish-to: {key}"),
                },
                other => other,
            })?;

        package_sets.push((
            pattern,
            PackageConfig {
                release_trigger,
                publish_to,
                tag_template,
                changelog,
                pre_major_inference,
            },
        ));
    }

    Ok(ResolvedConfig {
        root: root.to_path_buf(),
        changesets_dir,
        cascade: CascadeConfig {
            mode,
            bump_severity,
            peer_escalation,
            preserve_npm_ranges,
        },
        validation: ValidationConfig { allow_empty_changesets },
        registries,
        packages,
        package_sets,
        groups: GroupTable::default(),
        raw_groups,
        promoted_siblings: BTreeMap::new(),
        provenance,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn with_promoted_siblings_overlays_without_mutating_original() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join("callisto.toml"), "").expect("write callisto.toml");
        let cfg = load(root).expect("load should succeed");
        assert!(
            cfg.promoted_siblings.is_empty(),
            "promoted_siblings must default to empty at config-load time"
        );

        let mut local: BTreeMap<String, Vec<(PackageId, std::collections::BTreeSet<Ecosystem>)>> = BTreeMap::new();
        let mut ecos = std::collections::BTreeSet::new();
        ecos.insert(Ecosystem::Cargo);
        local.insert(
            "native-core".to_string(),
            vec![(
                PackageId::Prefixed {
                    ecosystem: Ecosystem::Cargo,
                    name: "native-core".to_string(),
                },
                ecos,
            )],
        );
        let overlay = cfg.with_promoted_siblings(local);
        assert_eq!(overlay.promoted_siblings.len(), 1);
        assert!(
            cfg.promoted_siblings.is_empty(),
            "original cfg must remain untouched by with_promoted_siblings"
        );
    }

    #[test]
    fn promoted_siblings_field_is_pub_not_pub_crate() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join("callisto.toml"), "").expect("write callisto.toml");
        let cfg = load(root).expect("load should succeed");
        let _: &BTreeMap<String, Vec<(PackageId, std::collections::BTreeSet<Ecosystem>)>> = &cfg.promoted_siblings;
    }

    #[test]
    fn test_config_resolve_rejects_traversal_in_changesets_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join("callisto.toml"), "[changesets]\ndir = \"../../tmp\"\n").expect("write callisto.toml");

        let result = load(root);
        assert!(
            result.is_err(),
            "expected load() to fail for traversal changesets dir, got Ok"
        );
        let err = result.unwrap_err();
        assert!(
            matches!(err, ConfigError::InvalidChangesetsDir { .. }),
            "expected InvalidChangesetsDir error, got: {err:?}"
        );
    }

    #[test]
    fn test_config_resolve_rejects_absolute_changesets_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join("callisto.toml"), "[changesets]\ndir = \"/etc\"\n").expect("write callisto.toml");

        let result = load(root);
        assert!(
            result.is_err(),
            "expected load() to fail for absolute changesets dir '/etc', got Ok \
             (the traversal guard only checks '..' components, not absolute paths — \
             this is the bug under test)"
        );
        let err = result.unwrap_err();
        assert!(
            matches!(err, ConfigError::InvalidChangesetsDir { .. }),
            "expected InvalidChangesetsDir error, got: {err:?}"
        );
    }

    #[test]
    fn test_config_resolve_rejects_absolute_changelog_in_package_block() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        fs::write(
            root.join("callisto.toml"),
            "[[package]]\nmatch = \"some-pkg\"\nchangelog = \"/tmp/should-not-be-written-here.md\"\n",
        )
        .expect("write callisto.toml");

        let result = load(root);
        assert!(
            result.is_err(),
            "expected load() to fail for absolute changelog path, got Ok \
             (raw_pkg.changelog is currently read straight into a PathBuf with no validation \
             — this is the bug under test)"
        );
        let err = result.unwrap_err();
        assert!(
            matches!(err, ConfigError::InvalidChangelogPath { .. }),
            "expected InvalidChangelogPath error, got: {err:?}"
        );
    }

    #[test]
    fn test_config_resolve_accepts_normal_changesets_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join("callisto.toml"), "[changesets]\ndir = \".changeset\"\n").expect("write callisto.toml");

        let result = load(root);
        assert!(
            result.is_ok(),
            "expected load() to succeed for normal changesets dir, got: {result:?}"
        );
    }

    #[test]
    fn test_package_set_is_parsed_into_resolved_config() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        fs::write(
            root.join("callisto.toml"),
            "[[package-set]]\nmatch = \"pkg-*\"\npublish-to = [\"none\"]\nrelease-trigger = \"auto\"\n",
        )
        .expect("write callisto.toml");

        let config = load(root).expect("load should succeed");
        assert_eq!(
            config.package_sets.len(),
            1,
            "one [[package-set]] rule expected; got: {:?}",
            config.package_sets
        );
        let (pattern, pkg_cfg) = &config.package_sets[0];
        assert!(
            pattern.matches(&callisto_model::PackageId::parse("pkg-a").unwrap()),
            "pattern 'pkg-*' must match 'pkg-a'"
        );
        assert_eq!(
            pkg_cfg.publish_to,
            Some(vec![callisto_model::PublishTarget::None]),
            "publish-to = [\"none\"] must be parsed"
        );
        assert_eq!(
            pkg_cfg.release_trigger,
            Some(callisto_model::ReleaseTrigger::Auto),
            "release-trigger = \"auto\" must be parsed"
        );
    }

    #[test]
    fn test_typo_in_callisto_toml_is_rejected_not_silently_ignored() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        // "cascade_mode" is a common typo for [cascade] mode = "always"
        fs::write(root.join("callisto.toml"), "[cascade]\ncascade_mode = \"always\"\n").expect("write callisto.toml");

        let result = load(root);
        assert!(
            result.is_err(),
            "load() must reject unknown field 'cascade_mode'; \
             silently ignoring it means the user's typo has no effect and they have no idea why"
        );
    }

    #[test]
    fn test_package_override_publish_to_is_parsed_not_discarded() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        fs::write(
            root.join("callisto.toml"),
            "[[package]]\nmatch = \"my-crate\"\npublish-to = [\"none\"]\n",
        )
        .expect("write callisto.toml");

        let config = load(root).expect("load should succeed");
        assert_eq!(config.packages.len(), 1, "one [[package]] rule expected");
        let (_, pkg_cfg) = &config.packages[0];
        assert!(
            pkg_cfg.publish_to.is_some(),
            "PackageConfig.publish_to must not be None when [[package]] publish-to is set;\
             got: {:?}",
            pkg_cfg.publish_to
        );
        let targets = pkg_cfg.publish_to.as_ref().unwrap();
        assert_eq!(
            targets,
            &vec![PublishTarget::None],
            "publish-to = [\"none\"] must produce [PublishTarget::None]; got: {targets:?}"
        );
    }

    #[test]
    fn test_registry_kind_pypi_resolves_to_pypi_ecosystem() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        fs::write(
            root.join("callisto.toml"),
            "[registries.my-pypi]\nkind = \"pypi\"\nurl = \"https://pypi.example.com/simple\"\n",
        )
        .expect("write callisto.toml");

        let config = load(root).expect("load should succeed");
        let reg = config
            .registries
            .get(&RegistryKey("my-pypi".to_string()))
            .expect("my-pypi registry should be present");
        assert_eq!(
            reg.kind,
            Ecosystem::Pypi,
            "registry with kind = \"pypi\" must resolve to Ecosystem::Pypi, not {:?}",
            reg.kind
        );
    }

    #[test]
    fn test_registry_kind_unknown_is_rejected() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        fs::write(
            root.join("callisto.toml"),
            "[registries.bad-registry]\nkind = \"maven\"\n",
        )
        .expect("write callisto.toml");

        let result = load(root);
        assert!(result.is_err(), "load() should fail for unknown registry kind, got Ok");
        assert!(
            matches!(result.unwrap_err(), ConfigError::UnknownKey { .. }),
            "expected UnknownKey error for unknown registry kind"
        );
    }

    /// AC-F1: Bare rule declared first, Prefixed rule declared second.
    /// Both rules set release_trigger with different values so the winner is directly observable.
    /// Bare foo: release-trigger = "auto" -> release_trigger = Some(Auto).
    /// Prefixed npm/foo: release-trigger = "changeset" -> release_trigger = Some(Changeset).
    /// Pass 1 finds npm/foo first (ecosystem is Some, name matches) -> result must be Some(Changeset).
    /// If the Bare rule incorrectly won: release_trigger == Some(Auto).
    #[test]
    fn resolve_package_config_prefixed_beats_bare_when_bare_declared_first() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        fs::write(
            root.join("callisto.toml"),
            "[[package]]\nmatch = \"foo\"\nrelease-trigger = \"auto\"\n\n[[package]]\nmatch = \"npm/foo\"\nrelease-trigger = \"changeset\"\n",
        )
        .expect("write callisto.toml");
        let cfg = load(root).expect("load should succeed");
        let id = PackageId::parse("foo").unwrap();
        let pcfg = resolve_package_config(&id, &cfg)
            .unwrap()
            .expect("resolve_package_config must return Some for npm/foo (Prefixed)");
        assert_eq!(
            pcfg.release_trigger,
            Some(ReleaseTrigger::Changeset),
            "Prefixed rule (npm/foo, changeset) must win over Bare rule (foo, auto) (AC-F1); \
             Some(Auto) means the Bare rule incorrectly won. Got: {:?}",
            pcfg.release_trigger
        );
    }

    /// AC-F2: Three-rule scenario. First Prefixed rule matches a different name.
    /// Pass 1 skips cargo/other (name mismatch), finds npm/pkg as the first Prefixed entry
    /// whose name matches, and returns it. The third entry (pypi/pkg) is never reached.
    #[test]
    fn resolve_package_config_first_matching_prefixed_wins_name_mismatch_skipped() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        // cargo/other: Prefixed but name "other" != "pkg" — skipped in pass 1
        // npm/pkg:     Prefixed, name matches — wins: changelog = "FIRST.md"
        // pypi/pkg:    Prefixed, name matches — never reached
        fs::write(
            root.join("callisto.toml"),
            "[[package]]\nmatch = \"cargo/other\"\nchangelog = \"WRONG.md\"\n\n[[package]]\nmatch = \"npm/pkg\"\nchangelog = \"FIRST.md\"\n\n[[package]]\nmatch = \"pypi/pkg\"\nchangelog = \"SECOND.md\"\n",
        )
        .expect("write callisto.toml");
        let cfg = load(root).expect("load should succeed");
        let id = PackageId::parse("pkg").unwrap();
        let pcfg = resolve_package_config(&id, &cfg)
            .unwrap()
            .expect("resolve_package_config must return Some for npm/pkg matching pkg");
        assert!(
            pcfg.changelog
                .as_ref()
                .map(|p| p.ends_with("FIRST.md"))
                .unwrap_or(false),
            "First MATCHING Prefixed entry (npm/pkg) must win (AC-F2): expected changelog ending \
             in FIRST.md. cargo/other skipped (name mismatch); pypi/pkg never reached. \
             Got changelog: {:?}",
            pcfg.changelog
        );
    }

    /// AC-F3: No Prefixed rule matches "pkg". Pass 1 finds nothing.
    /// Pass 2 finds the Bare "pkg" entry and returns it.
    #[test]
    fn resolve_package_config_bare_rule_wins_via_pass_2_when_no_prefixed_matches() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        // cargo/other: Prefixed but name "other" != "pkg" — pass 1 skips, pass 2 skips
        // pkg (Bare):  pass 1 skips (ecosystem() is None), pass 2 finds it — wins
        fs::write(
            root.join("callisto.toml"),
            "[[package]]\nmatch = \"cargo/other\"\nchangelog = \"WRONG.md\"\n\n[[package]]\nmatch = \"pkg\"\nrelease-trigger = \"auto\"\n",
        )
        .expect("write callisto.toml");
        let cfg = load(root).expect("load should succeed");
        let id = PackageId::parse("pkg").unwrap();
        let pcfg = resolve_package_config(&id, &cfg)
            .unwrap()
            .expect("resolve_package_config must return Some via pass 2 for Bare(\"pkg\")");
        assert_eq!(
            pcfg.release_trigger,
            Some(ReleaseTrigger::Auto),
            "Bare rule must win via pass 2 (AC-F3): expected release_trigger = Some(Auto). \
             Got: {:?}",
            pcfg.release_trigger
        );
    }

    /// AC-F4: No rule matches "pkg" at all.
    /// Pass 1 and pass 2 both iterate zero matching entries; result is None.
    #[test]
    fn resolve_package_config_returns_none_when_no_rule_matches() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        // Only a Prefixed rule for "other" — neither pass finds a match for "pkg".
        fs::write(
            root.join("callisto.toml"),
            "[[package]]\nmatch = \"cargo/other\"\nchangelog = \"WRONG.md\"\n",
        )
        .expect("write callisto.toml");
        let cfg = load(root).expect("load should succeed");
        let id = PackageId::parse("pkg").unwrap();
        let result = resolve_package_config(&id, &cfg).unwrap();
        assert!(
            result.is_none(),
            "resolve_package_config must return None when no rule matches (AC-F4); got Some(...)"
        );
    }

    /// AC-F4b: cfg.packages is empty. Both passes iterate zero entries.
    /// Every query returns None regardless of the id argument.
    #[test]
    fn resolve_package_config_returns_none_for_empty_packages() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        // No [[package]] sections at all — packages Vec is empty.
        fs::write(root.join("callisto.toml"), "").expect("write callisto.toml");
        let cfg = load(root).expect("load should succeed");
        assert!(
            cfg.packages.is_empty(),
            "fixture must produce an empty packages Vec (AC-F4b); got: {:?}",
            cfg.packages
        );
        for id_str in &["pkg", "npm/pkg", "cargo/pkg"] {
            let id = PackageId::parse(id_str).unwrap();
            let result = resolve_package_config(&id, &cfg).unwrap();
            assert!(
                result.is_none(),
                "resolve_package_config must return None for empty packages, \
                 id={id_str} (AC-F4b)"
            );
        }
    }

    /// AC-F5: Two Prefixed rules — npm/foo declared first, cargo/foo declared second in Vec order.
    /// Both rules set release_trigger with different values so the winner is directly observable.
    /// npm/foo (first): release-trigger = "auto" -> release_trigger = Some(Auto).
    /// cargo/foo (second): release-trigger = "changeset" -> release_trigger = Some(Changeset).
    /// Pass 1 returns the first matching Prefixed entry in Vec (TOML declaration) order.
    /// Vec order must govern: npm/foo is first -> release_trigger must be Some(Auto).
    /// A sorted implementation (cargo < npm alphabetically) would return Some(Changeset).
    #[test]
    fn resolve_package_config_vec_declaration_order_not_alphabetical_sort() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        // npm/foo FIRST in Vec: release-trigger = "auto"
        // cargo/foo SECOND in Vec: release-trigger = "changeset"
        // Querying Bare("foo"): pass 1 returns npm/foo (first Prefixed match) -> Some(Auto).
        // A sorted implementation would return cargo/foo (cargo < npm) -> Some(Changeset).
        fs::write(
            root.join("callisto.toml"),
            "[[package]]\nmatch = \"npm/foo\"\nrelease-trigger = \"auto\"\n\n[[package]]\nmatch = \"cargo/foo\"\nrelease-trigger = \"changeset\"\n",
        )
        .expect("write callisto.toml");
        let cfg = load(root).expect("load should succeed");
        let id = PackageId::parse("foo").unwrap();
        let pcfg = resolve_package_config(&id, &cfg)
            .unwrap()
            .expect("resolve_package_config must return Some for Bare(\"foo\")");
        assert_eq!(
            pcfg.release_trigger,
            Some(ReleaseTrigger::Auto),
            "npm/foo (declared first) must win over cargo/foo (declared second) — Vec order, \
             not alphabetical sort, governs first-match-wins (AC-F5). \
             A sorted implementation would incorrectly return Some(Changeset). Got: {:?}",
            pcfg.release_trigger
        );
    }

    #[test]
    fn resolve_package_config_returns_ambiguous_name_for_unprefixed_rule_matching_two_promoted_siblings() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        fs::write(
            root.join("callisto.toml"),
            "[[package]]\nmatch = \"native-core\"\nrelease-trigger = \"auto\"\n",
        )
        .expect("write callisto.toml");
        let mut cfg = load(root).expect("load should succeed");

        let cargo_id = PackageId::Prefixed {
            ecosystem: Ecosystem::Cargo,
            name: "native-core".to_string(),
        };
        let npm_id = PackageId::Prefixed {
            ecosystem: Ecosystem::Npm,
            name: "native-core".to_string(),
        };
        let mut cargo_set = BTreeSet::new();
        cargo_set.insert(Ecosystem::Cargo);
        let mut npm_set = BTreeSet::new();
        npm_set.insert(Ecosystem::Npm);
        cfg.promoted_siblings.insert(
            "native-core".to_string(),
            vec![(cargo_id.clone(), cargo_set), (npm_id, npm_set)],
        );

        let err = resolve_package_config(&cargo_id, &cfg).unwrap_err();
        match err {
            GraphError::AmbiguousName { name, candidates } => {
                assert_eq!(name, "native-core");
                assert_eq!(candidates.len(), 2);
            }
            other => panic!("expected AmbiguousName, got {other:?}"),
        }
    }

    // --- parse_publish_target / parse_release_trigger direct coverage ------

    #[test]
    fn parse_publish_target_parses_all_known_variants() {
        assert_eq!(parse_publish_target("crates-io").unwrap(), PublishTarget::CratesIo);
        assert_eq!(
            parse_publish_target("npm").unwrap(),
            PublishTarget::Npm {
                registry: None,
                access: None
            }
        );
        assert_eq!(
            parse_publish_target("pypi").unwrap(),
            PublishTarget::Pypi { index: None }
        );
        assert_eq!(
            parse_publish_target("nuget").unwrap(),
            PublishTarget::NuGet { source: None }
        );
        assert_eq!(
            parse_publish_target("github-release").unwrap(),
            PublishTarget::GitHubRelease
        );
        assert_eq!(parse_publish_target("none").unwrap(), PublishTarget::None);
    }

    #[test]
    fn parse_publish_target_rejects_unknown_string() {
        let result = parse_publish_target("bogus-registry");
        assert!(result.is_err(), "expected Err for unknown publish-to value, got Ok");
        assert!(
            matches!(result.unwrap_err(), ConfigError::UnknownKey { .. }),
            "expected UnknownKey error variant"
        );
    }

    #[test]
    fn parse_release_trigger_parses_both_known_variants() {
        assert_eq!(parse_release_trigger("changeset").unwrap(), ReleaseTrigger::Changeset);
        assert_eq!(parse_release_trigger("auto").unwrap(), ReleaseTrigger::Auto);
    }

    #[test]
    fn parse_release_trigger_rejects_unknown_string() {
        let result = parse_release_trigger("sometimes");
        assert!(
            result.is_err(),
            "expected Err for unknown release-trigger value, got Ok"
        );
        assert!(
            matches!(result.unwrap_err(), ConfigError::UnknownKey { .. }),
            "expected UnknownKey error variant"
        );
    }

    #[test]
    fn parse_pre_major_policy_parses_all_known_variants() {
        assert_eq!(parse_pre_major_policy("off").unwrap(), PreMajorInferencePolicy::Off);
        assert_eq!(parse_pre_major_policy("false").unwrap(), PreMajorInferencePolicy::Off);
        assert_eq!(
            parse_pre_major_policy("conservative").unwrap(),
            PreMajorInferencePolicy::Conservative
        );
        assert_eq!(
            parse_pre_major_policy("conservative-feat").unwrap(),
            PreMajorInferencePolicy::ConservativeFeat
        );
    }

    #[test]
    fn parse_pre_major_policy_rejects_unknown_string() {
        let result = parse_pre_major_policy("aggressive");
        assert!(
            matches!(result, Err(ConfigError::InvalidPreMajorInference { .. })),
            "expected InvalidPreMajorInference, got: {result:?}"
        );
    }

    /// End-to-end: `[[package]] publish-to = ["npm"|"pypi"|"nuget"|"github-release"]`
    /// must actually reach `PackageConfig.publish_to` through the full TOML load path,
    /// not just through calling `parse_publish_target` directly. Previously only
    /// "none"/"crates-io" were ever exercised through `load()`.
    #[test]
    fn package_block_parses_npm_pypi_nuget_github_release_publish_targets() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        fs::write(
            root.join("callisto.toml"),
            "[[package]]\nmatch = \"npm/pkg-a\"\npublish-to = [\"npm\"]\n\n\
             [[package]]\nmatch = \"pypi/pkg-b\"\npublish-to = [\"pypi\"]\n\n\
             [[package]]\nmatch = \"cargo/pkg-c\"\npublish-to = [\"nuget\"]\n\n\
             [[package]]\nmatch = \"cargo/pkg-d\"\npublish-to = [\"github-release\"]\n",
        )
        .expect("write callisto.toml");

        let cfg = load(root).expect("load should succeed");
        assert_eq!(cfg.packages.len(), 4);
        assert_eq!(
            cfg.packages[0].1.publish_to,
            Some(vec![PublishTarget::Npm {
                registry: None,
                access: None
            }]),
            "npm/pkg-a publish-to = [\"npm\"] must resolve to PublishTarget::Npm"
        );
        assert_eq!(
            cfg.packages[1].1.publish_to,
            Some(vec![PublishTarget::Pypi { index: None }]),
            "pypi/pkg-b publish-to = [\"pypi\"] must resolve to PublishTarget::Pypi"
        );
        assert_eq!(
            cfg.packages[2].1.publish_to,
            Some(vec![PublishTarget::NuGet { source: None }]),
            "cargo/pkg-c publish-to = [\"nuget\"] must resolve to PublishTarget::NuGet"
        );
        assert_eq!(
            cfg.packages[3].1.publish_to,
            Some(vec![PublishTarget::GitHubRelease]),
            "cargo/pkg-d publish-to = [\"github-release\"] must resolve to PublishTarget::GitHubRelease"
        );
    }

    /// An unknown `publish-to` value inside a `[[package]]` block must fail
    /// `load()` with a `[[package]] publish-to: ...`-prefixed `UnknownKey`
    /// error, not be silently dropped or panic.
    #[test]
    fn package_block_rejects_unknown_publish_to_value() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        fs::write(
            root.join("callisto.toml"),
            "[[package]]\nmatch = \"my-crate\"\npublish-to = [\"bogus\"]\n",
        )
        .expect("write callisto.toml");

        let result = load(root);
        assert!(
            result.is_err(),
            "expected load() to reject unknown publish-to value in [[package]] block, got Ok"
        );
        let err = result.unwrap_err();
        assert!(
            matches!(&err, ConfigError::UnknownKey { key, .. } if key.contains("[[package]] publish-to")),
            "expected UnknownKey error mentioning '[[package]] publish-to', got: {err:?}"
        );
    }

    /// An unknown `release-trigger` value inside a `[[package]]` block must
    /// fail `load()` rather than be silently dropped.
    #[test]
    fn package_block_rejects_unknown_release_trigger_value() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        fs::write(
            root.join("callisto.toml"),
            "[[package]]\nmatch = \"my-crate\"\nrelease-trigger = \"sometimes\"\n",
        )
        .expect("write callisto.toml");

        let result = load(root);
        assert!(
            result.is_err(),
            "expected load() to reject unknown release-trigger value in [[package]] block, got Ok"
        );
        assert!(
            matches!(result.unwrap_err(), ConfigError::UnknownKey { .. }),
            "expected UnknownKey error variant"
        );
    }
}

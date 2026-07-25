use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use callisto_model::{
    ConfigKey, Ecosystem, PackageId, PublishTarget, RegistryKey, ReleaseTrigger, Severity,
    TagTemplate,
};

use crate::config::groups::{GroupTable, RawGroupTable};
use crate::config::raw::RawConfig;
use crate::error::ConfigError;

#[derive(Clone, Debug)]
pub struct ResolvedConfig {
    pub root: PathBuf,
    pub changesets_dir: PathBuf,
    pub cascade: CascadeConfig,
    pub validation: ValidationConfig,
    pub registries: BTreeMap<RegistryKey, RegistryConfig>,
    pub packages: BTreeMap<PackageId, PackageConfig>,
    pub groups: GroupTable,
    provenance: BTreeMap<ConfigKey, ConfigProvenance>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigProvenance {
    Default,
    Explicit,
}

impl ResolvedConfig {
    pub fn provenance(&self, key: &ConfigKey) -> ConfigProvenance {
        self.provenance
            .get(key)
            .copied()
            .unwrap_or(ConfigProvenance::Default)
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

#[derive(Clone, Debug)]
pub struct PackageConfig {
    pub release_trigger: ReleaseTrigger,
    pub publish_to: Vec<PublishTarget>,
    pub tag_template: Option<TagTemplate>,
    pub changelog: Option<PathBuf>,
    pub pre_major_inference: PreMajorInferencePolicy,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PreMajorInferencePolicy {
    pub breaking_to_minor: bool,
    pub feat_to_patch: bool,
}

impl PreMajorInferencePolicy {
    pub const OFF: Self = Self {
        breaking_to_minor: false,
        feat_to_patch: false,
    };
}

pub fn parse_pre_major_policy(s: &str) -> Result<PreMajorInferencePolicy, ConfigError> {
    match s {
        "off" | "false" => Ok(PreMajorInferencePolicy::OFF),
        "conservative" => Ok(PreMajorInferencePolicy {
            breaking_to_minor: true,
            feat_to_patch: false,
        }),
        "conservative-feat" => Ok(PreMajorInferencePolicy {
            breaking_to_minor: true,
            feat_to_patch: true,
        }),
        _ => Err(ConfigError::InvalidPreMajorInference {
            found: s.to_string(),
        }),
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

    let changesets_dir = PathBuf::from(
        raw.changesets
            .as_ref()
            .and_then(|c| c.dir.as_deref())
            .unwrap_or(".changeset"),
    );

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
        provenance.insert(
            ConfigKey::CASCADE_PEER_ESCALATION,
            ConfigProvenance::Explicit,
        );
    }

    let preserve_npm_ranges = cascade_raw.preserve_npm_ranges.unwrap_or(true);
    if cascade_raw.preserve_npm_ranges.is_some() {
        provenance.insert(
            ConfigKey::CASCADE_PRESERVE_NPM_RANGES,
            ConfigProvenance::Explicit,
        );
    }

    let validation_raw = raw.validation.unwrap_or_default();
    let allow_empty_changesets = validation_raw.allow_empty_changesets.unwrap_or(false);
    if validation_raw.allow_empty_changesets.is_some() {
        provenance.insert(
            ConfigKey::VALIDATION_ALLOW_EMPTY_CHANGESETS,
            ConfigProvenance::Explicit,
        );
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
                Some("npm") => Ecosystem::Npm,
                _ => Ecosystem::Npm,
            };
            registries.insert(key, RegistryConfig { kind, url: reg.url });
        }
    }

    let raw_groups = RawGroupTable {
        fixed: raw.fixed_group.unwrap_or_default(),
        linked: raw.linked_group.unwrap_or_default(),
    };
    GroupTable::validate_syntactic(&raw_groups)?;

    Ok(ResolvedConfig {
        root: root.to_path_buf(),
        changesets_dir,
        cascade: CascadeConfig {
            mode,
            bump_severity,
            peer_escalation,
            preserve_npm_ranges,
        },
        validation: ValidationConfig {
            allow_empty_changesets,
        },
        registries,
        packages: BTreeMap::new(),
        groups: GroupTable::default(),
        provenance,
    })
}

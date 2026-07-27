use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use callisto_changelog::{ChangeSource, ChangelogEntry, ChangelogInput};
use callisto_format::{parse_changeset, Changeset};
use callisto_model::{BumpReason, CommandRunner, Diagnostic, PackageId, Severity, Version};

use crate::config::GroupTable;
use crate::config::{PreMajorInferencePolicy, ResolvedConfig};
use crate::error::GraphError;
use crate::infer::SeverityInference;
use crate::resolver::DependencyResolver;
use crate::tags::TagIndex;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedChangeset {
    pub path: PathBuf,
    pub id: String,
    pub changeset: Changeset,
}

#[derive(Clone, Debug, Default)]
pub struct Aggregation {
    pub severities: BTreeMap<PackageId, Severity>,
    pub reasons: BTreeMap<PackageId, BumpReason>,
    pub named_by: BTreeMap<PackageId, NamedBy>,
    pub consumed: Vec<PathBuf>,
    pub changelog_inputs: BTreeMap<PackageId, ChangelogInput>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamedBy {
    Changeset,
    Inference,
}

pub fn load_changesets(
    root: &Path,
    cfg: &ResolvedConfig,
) -> Result<Vec<LoadedChangeset>, GraphError> {
    let dir = root.join(&cfg.changesets_dir);
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let entries = fs::read_dir(&dir).map_err(|e| callisto_model::ManifestError::Read {
        path: dir.clone(),
        message: e.to_string(),
    })?;

    let mut files = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("md") {
            if let Some(file_name) = path.file_name().and_then(|s| s.to_str()) {
                if file_name != "README.md" && file_name != "config.json" && file_name != "pre.json"
                {
                    files.push(path);
                }
            }
        }
    }

    files.sort();

    let mut loaded = Vec::new();
    for path in files {
        let content =
            fs::read_to_string(&path).map_err(|e| callisto_model::ManifestError::Read {
                path: path.clone(),
                message: e.to_string(),
            })?;
        let changeset = parse_changeset(&content)?;
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let rel_path = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
        loaded.push(LoadedChangeset {
            path: rel_path,
            id: stem,
            changeset,
        });
    }

    Ok(loaded)
}

pub fn apply_pre_major(
    inferred: Severity,
    policy: PreMajorInferencePolicy,
    current: &Version,
    has_prior_release: bool,
) -> (Severity, bool) {
    if policy == PreMajorInferencePolicy::OFF {
        return (inferred, false);
    }
    if current.major() != Some(0) || current.minor() == Some(0) || !has_prior_release {
        return (inferred, false);
    }

    match (policy, inferred) {
        (p, Severity::Major) if p.breaking_to_minor => (Severity::Minor, true),
        (p, Severity::Minor) if p.feat_to_patch => (Severity::Patch, true),
        (_, s) => (s, false),
    }
}

pub fn aggregate<D, R, I>(
    graph: &D,
    config: &ResolvedConfig,
    _runner: &R,
    tags: &TagIndex,
    _pre: Option<&callisto_format::PreState>,
    inference: &I,
) -> Result<Aggregation, GraphError>
where
    D: DependencyResolver,
    R: CommandRunner,
    I: SeverityInference,
{
    let loaded = load_changesets(&config.root, config)?;
    let mut agg = Aggregation::default();

    for pkg in graph.packages() {
        let cur_sev = agg
            .severities
            .get(&pkg.id)
            .copied()
            .unwrap_or(Severity::None);
        let pathspecs: Vec<PathBuf> = pkg.manifests.iter().map(|m| m.path.clone()).collect();
        let last_tag = tags.last_tag(&pkg.id);
        let cur_ver = last_tag
            .map(|t| t.version.clone())
            .unwrap_or_else(|| Version::semver(1, 0, 0));

        let window = crate::infer::InferenceWindowSpec {
            pathspecs: &pathspecs,
            since: None,
            current_version: &cur_ver,
            has_prior_release: last_tag.is_some(),
            policy: PreMajorInferencePolicy::OFF,
        };

        if let Ok(Some(outcome)) = inference.infer(pkg, window) {
            if outcome.severity > cur_sev {
                agg.severities.insert(pkg.id.clone(), outcome.severity);
                agg.reasons.insert(
                    pkg.id.clone(),
                    BumpReason::Inference {
                        commits: outcome.commit_count,
                        remapped: outcome.remapped,
                    },
                );
                agg.named_by.insert(pkg.id.clone(), NamedBy::Inference);
            }
        }
    }

    for cs in loaded {
        agg.consumed.push(cs.path.clone());
        for entry in cs.changeset.entries {
            if let Ok(id) = PackageId::parse(&entry.name) {
                if graph.packages().any(|p| p.id == id) {
                    let cur_sev = agg.severities.get(&id).copied().unwrap_or(Severity::None);
                    if entry.severity > cur_sev {
                        agg.severities.insert(id.clone(), entry.severity);
                        agg.reasons.insert(
                            id.clone(),
                            BumpReason::Changeset {
                                changesets: vec![cs.id.clone()],
                            },
                        );
                        agg.named_by.insert(id.clone(), NamedBy::Changeset);
                    }

                    if entry.severity != Severity::None {
                        let pkg_ver = tags
                            .last_tag(&id)
                            .map(|t| t.version.clone())
                            .unwrap_or_else(|| Version::semver(0, 0, 0));
                        let cl_input =
                            agg.changelog_inputs.entry(id.clone()).or_insert_with(|| {
                                ChangelogInput {
                                    package: id.clone(),
                                    from: pkg_ver,
                                    to: None,
                                    entries: Vec::new(),
                                }
                            });
                        cl_input.entries.push(ChangelogEntry {
                            severity: entry.severity,
                            source: ChangeSource::Changeset {
                                filename: cs.id.clone(),
                                summary: cs.changeset.summary.clone(),
                            },
                        });
                    }
                }
            }
        }
    }

    union_fixed(&mut agg, &config.groups);
    union_linked(&mut agg, &config.groups);

    Ok(agg)
}

pub(crate) fn union_fixed(agg: &mut Aggregation, groups: &GroupTable) {
    for g in groups.fixed.values() {
        let pkg_members: Vec<PackageId> = g
            .members(crate::config::GroupMemberKind::Package)
            .filter_map(|m| match m {
                crate::config::GroupMember::Package(ref id) => Some(id.clone()),
                _ => None,
            })
            .collect();

        let mut target = Severity::None;
        for m in &pkg_members {
            if let Some(&s) = agg.severities.get(m) {
                if s > target {
                    target = s;
                }
            }
        }

        if target == Severity::None {
            continue;
        }

        for m in pkg_members {
            let cur = agg.severities.get(&m).copied().unwrap_or(Severity::None);
            if target > cur {
                agg.severities.insert(m.clone(), target);
                agg.reasons
                    .entry(m.clone())
                    .or_insert(BumpReason::FixedGroupUnion {
                        group: g.name.clone(),
                    });
            }
        }
    }
}

pub(crate) fn union_linked(agg: &mut Aggregation, groups: &GroupTable) {
    for g in groups.linked.values() {
        let named: Vec<PackageId> = g
            .members(crate::config::GroupMemberKind::Package)
            .filter_map(|m| match m {
                crate::config::GroupMember::Package(ref id) => {
                    if agg.named_by.contains_key(id) {
                        Some(id.clone())
                    } else {
                        None
                    }
                }
                _ => None,
            })
            .collect();

        if named.len() < 2 {
            continue;
        }

        let mut target_sev = Severity::None;
        for m in &named {
            if let Some(&s) = agg.severities.get(m) {
                if s > target_sev {
                    target_sev = s;
                }
            }
        }

        for m in named {
            let cur = agg.severities.get(&m).copied().unwrap_or(Severity::None);
            if target_sev > cur {
                agg.severities.insert(m.clone(), target_sev);
                agg.reasons.insert(
                    m.clone(),
                    BumpReason::LinkedGroupUnion {
                        group: g.name.clone(),
                    },
                );
            }
        }
    }
}

use std::io;

use callisto_graph::config::ResolvedConfig;
use callisto_model::{ComposePrBodyReport, InitReport, SnapshotReport, StatusReport, ValidateReport, VersionReport};

pub mod attribution;
pub mod diff;

/// `cfg` is `Some` only for report kinds that can actually carry a populated
/// `governed_by` today (currently just `VersionReport`, via [`render_version`]);
/// every other caller passes `None` so this stays a no-op for them rather than
/// forcing every render function to thread a `ResolvedConfig` it has no
/// governed diagnostic to attribute.
pub fn render_diagnostics<W: io::Write>(
    diagnostics: &[callisto_model::Diagnostic],
    cfg: Option<&ResolvedConfig>,
    w: &mut W,
) -> io::Result<()> {
    if !diagnostics.is_empty() {
        writeln!(w, "\nDiagnostics:")?;
        for d in diagnostics {
            writeln!(w, "  [{:?}] {}", d.severity, d.message)?;
            if let (Some(cfg), Some(key)) = (cfg, d.governed_by.as_ref()) {
                writeln!(w, "    {}", attribution::attribution_line(key, cfg))?;
            }
        }
    }
    Ok(())
}

pub fn render_status<W: io::Write>(report: &StatusReport, w: &mut W) -> io::Result<()> {
    writeln!(w, "Status (schema v{}):", report.schema_version)?;
    for pkg in &report.packages {
        let severity = pkg
            .pending_severity
            .map(|s| s.to_string())
            .unwrap_or_else(|| "none".to_string());
        writeln!(
            w,
            "  {} {} (pending: {})",
            pkg.package.display_name(),
            pkg.current_version.raw(),
            severity
        )?;
    }
    render_diagnostics(&report.diagnostics, None, w)
}

/// `cfg` is the resolved config that produced `report`: §13 invariant 28
/// requires the attribution line ("governed by ...") beneath any bump or
/// diagnostic whose default could defensibly have gone the other way, and
/// only the caller's already-resolved config has the value and provenance
/// that line needs (`callisto-graph` deliberately carries only the `ConfigKey`
/// in the report itself; see `render::attribution`).
pub fn render_version<W: io::Write>(report: &VersionReport, cfg: &ResolvedConfig, w: &mut W) -> io::Result<()> {
    writeln!(w, "Version Plan (schema v{}):", report.schema_version)?;
    for bump in &report.bumps {
        writeln!(
            w,
            "  {} {} → {}",
            bump.package.display_name(),
            bump.from.raw(),
            bump.to.raw()
        )?;
        if let Some(key) = bump.governed_by.as_ref() {
            writeln!(w, "    {}", attribution::attribution_line(key, cfg))?;
        }
    }
    render_diagnostics(&report.diagnostics, Some(cfg), w)
}

pub fn render_snapshot<W: io::Write>(report: &SnapshotReport, w: &mut W) -> io::Result<()> {
    writeln!(w, "Snapshot Tag: {}", report.snapshot_tag)?;
    for bump in &report.bumps {
        writeln!(
            w,
            "  {} {} → {}",
            bump.package.display_name(),
            bump.from.raw(),
            bump.to.raw()
        )?;
    }
    Ok(())
}

pub fn render_validate<W: io::Write>(report: &ValidateReport, w: &mut W) -> io::Result<()> {
    if report.ok {
        writeln!(w, "Validation passed.")?;
    } else {
        writeln!(w, "Validation failed with diagnostics:")?;
        render_diagnostics(&report.diagnostics, None, w)?;
    }
    Ok(())
}

pub fn render_compose_pr_body<W: io::Write>(report: &ComposePrBodyReport, w: &mut W) -> io::Result<()> {
    write!(w, "{}", report.body)?;
    Ok(())
}

pub fn render_init<W: io::Write>(report: &InitReport, w: &mut W) -> io::Result<()> {
    if report.initialized {
        writeln!(
            w,
            "Initialized callisto configuration at {}",
            report.config_path.display()
        )?;
    } else if report.diff.new_ecosystems.is_empty() {
        writeln!(
            w,
            "callisto configuration at {} is up to date; nothing to reconcile",
            report.config_path.display()
        )?;
    } else {
        let names: Vec<&str> = report.diff.new_ecosystems.iter().map(|e| e.prefix()).collect();
        if report.diff.applied {
            writeln!(
                w,
                "Reconciled {}: added newly-detected ecosystem(s) {}",
                report.config_path.display(),
                names.join(", ")
            )?;
        } else {
            writeln!(
                w,
                "Drift detected in {}: newly-detected ecosystem(s) {} — re-run with --yes to apply",
                report.config_path.display(),
                names.join(", ")
            )?;
        }
    }
    Ok(())
}

pub fn render_matrix<W: io::Write>(report: &callisto_model::MatrixReport, w: &mut W) -> io::Result<()> {
    writeln!(w, "Matrix (schema v{}):", report.schema_version)?;

    if report.platform_targets.is_empty() && report.runtime_versions.is_empty() {
        writeln!(w, "  (no platform targets or runtime-version constraints declared)")?;
    }

    for (pkg, group) in &report.platform_targets {
        writeln!(w, "  {pkg} [{:?} <- {}]:", group.kind, group.source)?;
        for t in &group.targets {
            writeln!(
                w,
                "    {:<32} abi={:<8} runner={:<14} cross={:<5} artifact={}",
                t.triple,
                t.abi.as_deref().unwrap_or("-"),
                t.host_runner,
                t.use_cross,
                t.artifact_name
            )?;
        }
    }

    for (pkg, entries) in &report.runtime_versions {
        for e in entries {
            writeln!(w, "  {pkg} [{:?}] {} = {}", e.ecosystem, e.field, e.range)?;
        }
    }

    render_diagnostics(&report.diagnostics, None, w)
}

#[cfg(test)]
mod tests {
    use super::*;
    use callisto_model::{
        BumpRecord, Ecosystem, PackageId, ReleaseTrigger, Severity, StatusPackageRecord, Version, VersionGrammar,
    };

    fn v1() -> Version {
        Version::parse("1.0.0", VersionGrammar::SemVer).unwrap()
    }

    fn pkg(name: &str) -> PackageId {
        PackageId::Prefixed {
            ecosystem: Ecosystem::Cargo,
            name: name.to_string(),
        }
    }

    fn status_pkg(name: &str, severity: Option<Severity>, changesets: Vec<&str>) -> StatusPackageRecord {
        StatusPackageRecord {
            package: pkg(name),
            current_version: v1(),
            last_tag: None,
            last_released_version: None,
            pending_severity: severity,
            changed_since_last_tag: false,
            release_trigger: ReleaseTrigger::Changeset,
            pending_changesets: changesets.into_iter().map(|s| s.to_string()).collect(),
        }
    }

    // QW-2: render_status must not produce "Some(" in output.
    #[test]
    fn render_status_no_some_wrapper_in_output() {
        let report = StatusReport {
            schema_version: callisto_model::SCHEMA_VERSION,
            has_changesets: true,
            packages: vec![status_pkg("crate-a", Some(Severity::Minor), vec!["cs-001"])],
            diagnostics: vec![],
        };
        let mut out = Vec::new();
        render_status(&report, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            !text.contains("Some("),
            "render_status output must not contain 'Some('; got: {text}"
        );
        assert!(
            text.contains("minor"),
            "render_status output should contain severity 'minor'; got: {text}"
        );
    }

    // QW-9: render_publish with empty plan should say "nothing to publish".
    /// AC-008: text-format output for a report with at least one platform
    /// target and one runtime-version entry must be non-empty and must not
    /// parse as JSON.
    #[test]
    fn render_matrix_produces_non_json_non_empty_output() {
        use callisto_model::{
            MatrixReport, PlatformTarget, PlatformTargetGroup, PlatformTargetKind, RuntimeEcosystem,
            RuntimeVersionEntry,
        };
        use std::collections::BTreeMap;

        let mut platform_targets = BTreeMap::new();
        platform_targets.insert(
            "native-mod".to_string(),
            PlatformTargetGroup {
                kind: PlatformTargetKind::Napi,
                source: "napi.targets".to_string(),
                targets: vec![PlatformTarget {
                    triple: "aarch64-apple-darwin".to_string(),
                    platform: "darwin".to_string(),
                    arch: "arm64".to_string(),
                    abi: None,
                    host_runner: "macos-latest".to_string(),
                    use_cross: false,
                    artifact_name: "native-mod-darwin-arm64".to_string(),
                    package_dir: "native-mod".to_string(),
                    package_name: "native-mod".to_string(),
                }],
            },
        );
        let mut runtime_versions = BTreeMap::new();
        runtime_versions.insert(
            "native-mod".to_string(),
            vec![RuntimeVersionEntry {
                ecosystem: RuntimeEcosystem::Npm,
                field: "engines.node".to_string(),
                range: ">=20.0.0".to_string(),
            }],
        );
        let report = MatrixReport {
            schema_version: 1,
            platform_targets,
            runtime_versions,
            diagnostics: vec![],
        };

        let mut buf = Vec::new();
        render_matrix(&report, &mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();

        assert!(!text.is_empty(), "table output must not be empty");
        assert!(
            serde_json::from_str::<serde_json::Value>(&text).is_err(),
            "table output must not itself parse as JSON: {text}"
        );
        assert!(
            text.contains("native-mod"),
            "table must mention the package name: {text}"
        );
        assert!(
            text.contains("aarch64-apple-darwin"),
            "table must mention the triple: {text}"
        );
    }

    /// AC-011: render_matrix must surface a diagnostic's triple/message in
    /// the human-readable table output, not just its presence.
    #[test]
    fn render_matrix_renders_diagnostics_for_unrecognised_triple() {
        use callisto_model::{Diagnostic, DiagnosticCode, DiagnosticSeverity, MatrixReport, PackageId};
        use std::collections::BTreeMap;

        let report = MatrixReport {
            schema_version: 1,
            platform_targets: BTreeMap::new(),
            runtime_versions: BTreeMap::new(),
            diagnostics: vec![Diagnostic {
                code: DiagnosticCode::UnrecognisedPlatformTriple,
                severity: DiagnosticSeverity::Warning,
                message: "package `native-mod` declares unrecognised platform triple `sparc64-unknown-linux-gnu` in `napi.targets`".to_string(),
                package: Some(PackageId::Bare("native-mod".to_string())),
                path: None,
                escalated_by: None,
                governed_by: None,
            }],
        };

        let mut buf = Vec::new();
        render_matrix(&report, &mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();

        assert!(
            text.contains("sparc64-unknown-linux-gnu"),
            "table output must mention the unrecognised triple: {text}"
        );
        assert!(
            text.contains("native-mod"),
            "table output must mention the offending package: {text}"
        );
    }

    #[test]
    fn render_snapshot_lists_snapshot_tag_and_bumps() {
        let report = SnapshotReport {
            schema_version: callisto_model::SCHEMA_VERSION,
            snapshot_tag: "0.0.0-canary-abc1234".to_string(),
            bumps: vec![BumpRecord {
                package: pkg("crate-a"),
                from: v1(),
                to: Version::parse("0.0.0-canary-abc1234", VersionGrammar::SemVer).unwrap(),
                severity: Severity::Patch,
                governed_by: None,
                reason: None,
            }],
            diagnostics: vec![],
        };
        let mut out = Vec::new();
        render_snapshot(&report, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("0.0.0-canary-abc1234"), "got: {text}");
        assert!(text.contains("crate-a"), "got: {text}");
    }

    #[test]
    fn render_validate_ok_reports_pass() {
        let report = ValidateReport {
            schema_version: callisto_model::SCHEMA_VERSION,
            ok: true,
            diagnostics: vec![],
        };
        let mut out = Vec::new();
        render_validate(&report, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("Validation passed"), "got: {text}");
    }

    #[test]
    fn render_validate_failure_lists_diagnostics() {
        use callisto_model::{Diagnostic, DiagnosticCode, DiagnosticSeverity};

        let report = ValidateReport {
            schema_version: callisto_model::SCHEMA_VERSION,
            ok: false,
            diagnostics: vec![Diagnostic {
                code: DiagnosticCode::UnrecognisedPlatformTriple,
                severity: DiagnosticSeverity::Error,
                message: "something is wrong".to_string(),
                package: None,
                path: None,
                escalated_by: None,
                governed_by: None,
            }],
        };
        let mut out = Vec::new();
        render_validate(&report, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("Validation failed"), "got: {text}");
        assert!(text.contains("something is wrong"), "got: {text}");
    }

    #[test]
    fn render_init_up_to_date_reports_nothing_to_reconcile() {
        let report = InitReport {
            schema_version: callisto_model::SCHEMA_VERSION,
            initialized: false,
            config_path: std::path::PathBuf::from("callisto.toml"),
            diff: callisto_model::InitDiff {
                new_ecosystems: vec![],
                applied: false,
            },
            diagnostics: vec![],
        };
        let mut out = Vec::new();
        render_init(&report, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("up to date"), "got: {text}");
    }

    #[test]
    fn render_init_applied_drift_reports_reconciled() {
        let report = InitReport {
            schema_version: callisto_model::SCHEMA_VERSION,
            initialized: false,
            config_path: std::path::PathBuf::from("callisto.toml"),
            diff: callisto_model::InitDiff {
                new_ecosystems: vec![Ecosystem::Npm],
                applied: true,
            },
            diagnostics: vec![],
        };
        let mut out = Vec::new();
        render_init(&report, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("Reconciled"), "got: {text}");
        assert!(text.contains("npm"), "got: {text}");
    }

    #[test]
    fn render_init_unapplied_drift_reports_needs_yes_flag() {
        let report = InitReport {
            schema_version: callisto_model::SCHEMA_VERSION,
            initialized: false,
            config_path: std::path::PathBuf::from("callisto.toml"),
            diff: callisto_model::InitDiff {
                new_ecosystems: vec![Ecosystem::Npm],
                applied: false,
            },
            diagnostics: vec![],
        };
        let mut out = Vec::new();
        render_init(&report, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("Drift detected"), "got: {text}");
        assert!(text.contains("--yes"), "got: {text}");
    }

    /// §13 invariant 28 / §CLI.5.2: `render_version` must call
    /// `render::attribution` for every bump and diagnostic that carries a
    /// `governed_by`, and must stay silent for the ones that don't.
    #[test]
    fn render_version_prints_attribution_for_governed_bump_and_diagnostic() {
        use callisto_model::{ConfigKey, Diagnostic, DiagnosticCode, DiagnosticSeverity};

        let tmp = tempfile::tempdir().unwrap();
        let cfg = callisto_graph::config::load(tmp.path()).unwrap();

        let report = VersionReport {
            schema_version: callisto_model::SCHEMA_VERSION,
            bumps: vec![
                BumpRecord {
                    package: pkg("crate-a"),
                    from: v1(),
                    to: v1(),
                    severity: Severity::Patch,
                    governed_by: Some(ConfigKey::CASCADE_BUMP_SEVERITY),
                    reason: None,
                },
                BumpRecord {
                    package: pkg("crate-b"),
                    from: v1(),
                    to: v1(),
                    severity: Severity::Patch,
                    governed_by: None,
                    reason: None,
                },
            ],
            lockfile_refresh_results: None,
            diagnostics: vec![Diagnostic {
                code: DiagnosticCode::EmptyChangeset,
                severity: DiagnosticSeverity::Warning,
                message: "No pending changesets found in workspace".to_string(),
                package: None,
                path: None,
                escalated_by: None,
                governed_by: Some(ConfigKey::VALIDATION_ALLOW_EMPTY_CHANGESETS),
            }],
        };

        let mut out = Vec::new();
        render_version(&report, &cfg, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();

        assert!(
            text.contains("crate-a") && text.contains("governed by [cascade].bump-severity = patch (default)"),
            "governed bump must be followed by its attribution line; got:\n{text}"
        );
        assert_eq!(
            text.matches("governed by [cascade]").count(),
            1,
            "the ungoverned crate-b bump must not print an attribution line; got:\n{text}"
        );
        assert!(
            text.contains("governed by [validation].allow-empty-changesets = false (default)"),
            "governed diagnostic must be followed by its attribution line; got:\n{text}"
        );
    }
}

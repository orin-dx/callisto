use std::io;

use callisto_model::{
    ComposePrBodyReport, InitReport, PublishAttemptResult, PublishPlan, PublishReport, SnapshotReport, StatusReport,
    TagReport, ValidateReport, VersionReport,
};

pub mod attribution;
pub mod diff;

pub fn render_diagnostics<W: io::Write>(diagnostics: &[callisto_model::Diagnostic], w: &mut W) -> io::Result<()> {
    if !diagnostics.is_empty() {
        writeln!(w, "\nDiagnostics:")?;
        for d in diagnostics {
            writeln!(w, "  [{:?}] {}", d.severity, d.message)?;
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
    render_diagnostics(&report.diagnostics, w)
}

pub fn render_version<W: io::Write>(report: &VersionReport, w: &mut W) -> io::Result<()> {
    writeln!(w, "Version Plan (schema v{}):", report.schema_version)?;
    for bump in &report.bumps {
        writeln!(
            w,
            "  {} {} → {}",
            bump.package.display_name(),
            bump.from.raw(),
            bump.to.raw()
        )?;
    }
    render_diagnostics(&report.diagnostics, w)
}

pub fn render_publish<W: io::Write>(report: &PublishPlan, w: &mut W) -> io::Result<()> {
    let total_packages = report.rust_crates.len()
        + report.npm_platform_packages.len()
        + report.npm_main_packages.len()
        + report.pypi_packages.len();
    writeln!(w, "Publish Plan (schema v{}):", report.schema_version)?;
    for rel in &report.releases {
        writeln!(w, "  Tag: {} (sha: {})", rel.tag_name, rel.sha.as_str())?;
    }
    if total_packages == 0 && report.releases.is_empty() {
        writeln!(w, "  No packages to publish.")?;
        render_diagnostics(&report.diagnostics, w)?;
        return Ok(());
    }
    if total_packages == 0 {
        render_diagnostics(&report.diagnostics, w)?;
        return Ok(());
    }
    if !report.rust_crates.is_empty() {
        writeln!(w, "  Crates ({}):", report.rust_crates.len())?;
        for pkg in &report.rust_crates {
            writeln!(w, "    {} {}", pkg.name, pkg.version.raw())?;
        }
    }
    if !report.npm_main_packages.is_empty() {
        writeln!(w, "  npm packages ({}):", report.npm_main_packages.len())?;
        for pkg in &report.npm_main_packages {
            writeln!(w, "    {} {}", pkg.name, pkg.version.raw())?;
        }
    }
    if !report.npm_platform_packages.is_empty() {
        writeln!(w, "  npm platform packages ({}):", report.npm_platform_packages.len())?;
        for pkg in &report.npm_platform_packages {
            writeln!(w, "    {} {}", pkg.name, pkg.version.raw())?;
        }
    }
    if !report.pypi_packages.is_empty() {
        writeln!(w, "  PyPI packages ({}):", report.pypi_packages.len())?;
        for pkg in &report.pypi_packages {
            writeln!(w, "    {} {}", pkg.name, pkg.version.raw())?;
        }
    }
    render_diagnostics(&report.diagnostics, w)?;
    Ok(())
}

pub fn render_publish_report<W: io::Write>(report: &PublishReport, w: &mut W) -> io::Result<()> {
    writeln!(w, "Publish Report (schema v{}):", report.schema_version)?;
    for attempt in &report.attempts {
        let status = match &attempt.result {
            PublishAttemptResult::Published => "published".to_string(),
            PublishAttemptResult::AlreadyPublished => "already published".to_string(),
            PublishAttemptResult::Failed { kind, error } => format!("FAILED [{kind}]: {error}"),
        };
        writeln!(
            w,
            "  {} {} — {}",
            attempt.package.display_name(),
            attempt.version.raw(),
            status
        )?;
    }
    render_diagnostics(&report.diagnostics, w)
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
        for diag in &report.diagnostics {
            writeln!(w, "  [{:?}] {}", diag.severity, diag.message)?;
        }
    }
    Ok(())
}

pub fn render_tag<W: io::Write>(report: &TagReport, dry_run: bool, w: &mut W) -> io::Result<()> {
    if dry_run {
        writeln!(w, "Would create tags:")?;
    } else {
        writeln!(w, "Created Tags:")?;
    }
    for tag in &report.tags {
        writeln!(w, "  {} ({})", tag.tag_name, tag.sha.as_str())?;
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

    render_diagnostics(&report.diagnostics, w)
}

#[cfg(test)]
mod tests {
    use super::*;
    use callisto_model::{
        BumpRecord, CreatedTag, Ecosystem, PackageId, PublishAttempt, ReleaseTrigger, Severity, StatusPackageRecord,
        Version, VersionGrammar,
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

    fn mixed_report() -> PublishReport {
        PublishReport {
            schema_version: callisto_model::SCHEMA_VERSION,
            attempts: vec![
                PublishAttempt {
                    package: pkg("crate-a"),
                    version: v1(),
                    result: PublishAttemptResult::Published,
                },
                PublishAttempt {
                    package: pkg("crate-b"),
                    version: v1(),
                    result: PublishAttemptResult::AlreadyPublished,
                },
                PublishAttempt {
                    package: pkg("crate-c"),
                    version: v1(),
                    result: PublishAttemptResult::Failed {
                        kind: "authFailed".to_string(),
                        error: "auth failed: bad token".to_string(),
                    },
                },
            ],
            diagnostics: vec![],
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

    fn full_plan() -> PublishPlan {
        use callisto_model::{
            CratePublish, NpmMainPublish, NpmPublish, PypiPublish, RegistryKey, Version, SCHEMA_VERSION,
        };
        let v = Version::parse("1.0.0", callisto_model::VersionGrammar::SemVer).unwrap();
        PublishPlan {
            schema_version: SCHEMA_VERSION,
            rust_crates: vec![CratePublish {
                name: "my-crate".to_string(),
                version: v.clone(),
                publish_to: RegistryKey(RegistryKey::CRATES_IO.to_string()),
                registry: None,
                package_dir: None,
            }],
            npm_main_packages: vec![NpmMainPublish {
                name: "@scope/main-pkg".to_string(),
                version: v.clone(),
                publish_to: RegistryKey(RegistryKey::NPM.to_string()),
                registry: None,
                tag: None,
                access: None,
                depends_on_platforms: vec![],
                package_dir: std::path::PathBuf::new(),
            }],
            npm_platform_packages: vec![NpmPublish {
                name: "@scope/main-pkg-linux-x64-gnu".to_string(),
                version: v.clone(),
                publish_to: RegistryKey(RegistryKey::NPM.to_string()),
                registry: None,
                tag: None,
                access: None,
                package_dir: std::path::PathBuf::new(),
            }],
            pypi_packages: vec![PypiPublish {
                name: "my-pypi-pkg".to_string(),
                version: v,
                publish_to: RegistryKey(RegistryKey::PYPI.to_string()),
                index: None,
                package_dir: std::path::PathBuf::new(),
            }],
            releases: vec![],
            diagnostics: vec![],
        }
    }

    #[test]
    fn render_publish_lists_all_four_package_types() {
        let plan = full_plan();
        let mut out = Vec::new();
        render_publish(&plan, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("my-crate"),
            "render_publish must list rust crates; got:\n{text}"
        );
        assert!(
            text.contains("@scope/main-pkg"),
            "render_publish must list npm main packages; got:\n{text}"
        );
        assert!(
            text.contains("@scope/main-pkg-linux-x64-gnu"),
            "render_publish must list npm platform packages; got:\n{text}"
        );
        assert!(
            text.contains("my-pypi-pkg"),
            "render_publish must list pypi packages; got:\n{text}"
        );
    }

    // QW-9: render_publish with empty plan should say "nothing to publish".
    #[test]
    fn render_publish_empty_plan_shows_nothing_to_publish() {
        let plan = PublishPlan {
            schema_version: callisto_model::SCHEMA_VERSION,
            rust_crates: vec![],
            npm_platform_packages: vec![],
            npm_main_packages: vec![],
            pypi_packages: vec![],
            releases: vec![],
            diagnostics: vec![],
        };
        let mut out = Vec::new();
        render_publish(&plan, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap().to_lowercase();
        assert!(
            text.contains("no packages") || text.contains("nothing to publish"),
            "render_publish empty plan must mention 'no packages' or 'nothing to publish'; got: {text}"
        );
    }

    /// Diagnostics emitted during plan computation (e.g. GitDiscoveryFailed,
    /// ChangesetReadError) must appear in the text output of render_publish.
    /// Without this, an operator using `plan-publish --format text` gets no
    /// explanation when releases are silently omitted due to a git error.
    #[test]
    fn render_publish_surfaces_plan_diagnostics() {
        use callisto_model::{Diagnostic, DiagnosticCode, DiagnosticSeverity};

        let mut plan = PublishPlan {
            schema_version: callisto_model::SCHEMA_VERSION,
            rust_crates: vec![],
            npm_platform_packages: vec![],
            npm_main_packages: vec![],
            pypi_packages: vec![],
            releases: vec![],
            diagnostics: vec![Diagnostic {
                code: DiagnosticCode::GitDiscoveryFailed,
                severity: DiagnosticSeverity::Warning,
                message: "could not discover git repository: not a git repo".to_string(),
                package: None,
                path: None,
                escalated_by: None,
                governed_by: None,
            }],
        };

        use callisto_model::{CratePublish, RegistryKey};

        // Non-empty plan case: diagnostic must appear alongside package list.
        plan.rust_crates.push(CratePublish {
            name: "my-crate".to_string(),
            version: v1(),
            publish_to: RegistryKey(RegistryKey::CRATES_IO.to_string()),
            registry: None,
            package_dir: None,
        });
        let mut out = Vec::new();
        render_publish(&plan, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.to_ascii_lowercase().contains("git") || text.contains("GitDiscoveryFailed"),
            "render_publish must include diagnostic text; got:\n{text}"
        );

        // Empty-plan case: diagnostic must appear even when no packages are listed.
        plan.rust_crates.clear();
        let mut out2 = Vec::new();
        render_publish(&plan, &mut out2).unwrap();
        let text2 = String::from_utf8(out2).unwrap();
        assert!(
            text2.to_ascii_lowercase().contains("git") || text2.contains("GitDiscoveryFailed"),
            "render_publish must include diagnostic text even for empty plan; got:\n{text2}"
        );
    }

    #[test]
    fn render_publish_report_text_distinguishes_per_package_outcomes() {
        let mut out = Vec::new();
        render_publish_report(&mixed_report(), &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();

        assert!(text.contains("crate-a") && text.contains("published"));
        assert!(text.contains("crate-b") && text.contains("already published"));
        assert!(text.contains("crate-c") && text.contains("FAILED [authFailed]: auth failed: bad token"));
    }

    /// The text renderer must surface the `kind` discriminator so operators can
    /// distinguish "authFailed" (permanent — rotate credentials) from
    /// "rateLimited" (transient — safe to retry) without parsing the human-
    /// readable error string.
    #[test]
    fn render_publish_report_failed_includes_error_kind() {
        use callisto_model::{PublishAttempt, PublishReport, SCHEMA_VERSION};

        let report = PublishReport {
            schema_version: SCHEMA_VERSION,
            attempts: vec![
                PublishAttempt {
                    package: pkg("pkg-a"),
                    version: v1(),
                    result: callisto_model::PublishAttemptResult::Failed {
                        kind: "authFailed".to_string(),
                        error: "invalid token".to_string(),
                    },
                },
                PublishAttempt {
                    package: pkg("pkg-b"),
                    version: v1(),
                    result: callisto_model::PublishAttemptResult::Failed {
                        kind: "rateLimited".to_string(),
                        error: "try again in 60s".to_string(),
                    },
                },
            ],
            diagnostics: vec![],
        };

        let mut out = Vec::new();
        render_publish_report(&report, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();

        assert!(
            text.contains("authFailed"),
            "text output must include the error kind 'authFailed' so operators \
             can distinguish it from transient failures; got:\n{text}"
        );
        assert!(
            text.contains("rateLimited"),
            "text output must include the error kind 'rateLimited'; got:\n{text}"
        );
    }

    #[test]
    fn publish_report_json_distinguishes_per_package_outcomes() {
        let json = serde_json::to_string(&mixed_report()).unwrap();

        assert!(json.contains("\"status\":\"published\""));
        assert!(json.contains("\"status\":\"alreadyPublished\""));
        assert!(json.contains("\"status\":\"failed\"") && json.contains("auth failed: bad token"));
    }

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
    fn render_tag_dry_run_vs_real_use_distinct_headers() {
        use callisto_model::{CommitSha, TagName};

        let report = TagReport {
            schema_version: callisto_model::SCHEMA_VERSION,
            tags: vec![CreatedTag {
                package: pkg("crate-a"),
                tag_name: TagName("crate-a@1.0.0".to_string()),
                sha: CommitSha::parse(&"a".repeat(40)).unwrap(),
                already_existed: false,
                is_floating_major: false,
            }],
            diagnostics: vec![],
        };

        let mut dry_run_out = Vec::new();
        render_tag(&report, true, &mut dry_run_out).unwrap();
        let dry_run_text = String::from_utf8(dry_run_out).unwrap();
        assert!(dry_run_text.contains("Would create tags"), "got: {dry_run_text}");
        assert!(dry_run_text.contains("crate-a@1.0.0"), "got: {dry_run_text}");

        let mut real_out = Vec::new();
        render_tag(&report, false, &mut real_out).unwrap();
        let real_text = String::from_utf8(real_out).unwrap();
        assert!(real_text.contains("Created Tags"), "got: {real_text}");
        assert!(!real_text.contains("Would create"), "got: {real_text}");
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
}

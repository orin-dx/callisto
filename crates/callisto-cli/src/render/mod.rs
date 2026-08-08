use std::io;

use callisto_model::{
    ComposePrBodyReport, InitReport, PublishAttemptResult, PublishPlan, PublishReport,
    SnapshotReport, StatusReport, TagReport, ValidateReport, VersionReport,
};

pub mod attribution;
pub mod diff;

pub fn render_diagnostics<W: io::Write>(
    diagnostics: &[callisto_model::Diagnostic],
    w: &mut W,
) -> io::Result<()> {
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
    if total_packages == 0 {
        writeln!(w, "  No packages to publish.")?;
        return Ok(());
    }
    for rel in &report.releases {
        writeln!(w, "  Tag: {} (sha: {})", rel.tag_name, rel.sha.as_str())?;
    }
    Ok(())
}

pub fn render_publish_report<W: io::Write>(report: &PublishReport, w: &mut W) -> io::Result<()> {
    writeln!(w, "Publish Report (schema v{}):", report.schema_version)?;
    for attempt in &report.attempts {
        let status = match &attempt.result {
            PublishAttemptResult::Published => "published".to_string(),
            PublishAttemptResult::AlreadyPublished => "already published".to_string(),
            PublishAttemptResult::Failed { error } => format!("FAILED: {error}"),
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
    if report.valid {
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
    for tag in &report.created_tags {
        writeln!(w, "  {} ({})", tag.tag_name, tag.sha.as_str())?;
    }
    Ok(())
}

pub fn render_compose_pr_body<W: io::Write>(
    report: &ComposePrBodyReport,
    w: &mut W,
) -> io::Result<()> {
    write!(w, "{}", report.pr_body)?;
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
        let names: Vec<&str> = report
            .diff
            .new_ecosystems
            .iter()
            .map(|e| e.prefix())
            .collect();
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

#[cfg(test)]
mod tests {
    use super::*;
    use callisto_model::{
        Ecosystem, PackageId, PublishAttempt, Severity, StatusPackageRecord, Version,
        VersionGrammar,
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
            schema_version: 1,
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
                        error: "auth failed: bad token".to_string(),
                    },
                },
            ],
            diagnostics: vec![],
        }
    }

    fn status_pkg(
        name: &str,
        severity: Option<Severity>,
        changesets: Vec<&str>,
    ) -> StatusPackageRecord {
        StatusPackageRecord {
            package: pkg(name),
            current_version: v1(),
            last_tag: None,
            pending_severity: severity,
            changed_since_last_tag: false,
            pending_changesets: changesets.into_iter().map(|s| s.to_string()).collect(),
        }
    }

    // QW-2: render_status must not produce "Some(" in output.
    #[test]
    fn render_status_no_some_wrapper_in_output() {
        let report = StatusReport {
            schema_version: 1,
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
    #[test]
    fn render_publish_empty_plan_shows_nothing_to_publish() {
        let plan = PublishPlan {
            schema_version: 1,
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

    #[test]
    fn render_publish_report_text_distinguishes_per_package_outcomes() {
        let mut out = Vec::new();
        render_publish_report(&mixed_report(), &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();

        assert!(text.contains("crate-a") && text.contains("published"));
        assert!(text.contains("crate-b") && text.contains("already published"));
        assert!(text.contains("crate-c") && text.contains("FAILED: auth failed: bad token"));
    }

    #[test]
    fn publish_report_json_distinguishes_per_package_outcomes() {
        let json = serde_json::to_string(&mixed_report()).unwrap();

        assert!(json.contains("\"status\":\"published\""));
        assert!(json.contains("\"status\":\"alreadyPublished\""));
        assert!(json.contains("\"status\":\"failed\"") && json.contains("auth failed: bad token"));
    }
}

use std::io;

use callisto_model::{
    ComposePrBodyReport, InitReport, PublishPlan, SnapshotReport, StatusReport, TagReport,
    ValidateReport, VersionReport,
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
        writeln!(
            w,
            "  {} {} (pending: {:?})",
            pkg.package.display_name(),
            pkg.current_version.raw(),
            pkg.pending_severity
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
    writeln!(w, "Publish Plan (schema v{}):", report.schema_version)?;
    for rel in &report.releases {
        writeln!(w, "  Tag: {} (sha: {})", rel.tag_name, rel.sha.as_str())?;
    }
    Ok(())
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

pub fn render_tag<W: io::Write>(report: &TagReport, w: &mut W) -> io::Result<()> {
    writeln!(w, "Created Tags:")?;
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
    writeln!(
        w,
        "Initialized callisto configuration at {}",
        report.config_path.display()
    )?;
    Ok(())
}

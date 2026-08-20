use callisto_model::{CommandRunner, ValidateReport, SCHEMA_VERSION};

use crate::commands::escalate;
use crate::error::GraphError;
use crate::resolver::DependencyResolver;
use crate::Workspace;

#[derive(Clone, Debug, Default)]
pub struct ValidateOptions {
    pub staged: bool,
    pub since: Option<String>,
    pub strict: bool,
    pub strict_graph: bool,
}

pub fn validate<R: CommandRunner, D: DependencyResolver>(
    ws: &Workspace<'_, R, D>,
    opts: &ValidateOptions,
) -> Result<ValidateReport, GraphError> {
    let mut diagnostics = Vec::new();
    let loaded = crate::load_changesets(&ws.root, &ws.config)?;

    let target_changesets: Vec<_> = if opts.staged {
        let out = ws
            .runner
            .run("git", &["diff", "--cached", "--name-only"], &ws.root)?;
        if !out.success() {
            return Err(GraphError::Command(callisto_model::CommandError::Io {
                program: "git".to_string(),
                message: crate::apply::redact_git_stderr(&out.stderr),
            }));
        }
        let files: Vec<String> = out
            .stdout_trimmed()
            .lines()
            .map(|l| l.trim_matches('"').to_string())
            .collect();
        loaded
            .into_iter()
            .filter(|cs| files.iter().any(|f| cs.path.ends_with(f)))
            .collect()
    } else if let Some(ref since) = opts.since {
        if since.starts_with('-') {
            return Err(GraphError::Command(callisto_model::CommandError::Io {
                program: "git".to_string(),
                message: "invalid since ref".to_string(),
            }));
        }
        let range = format!("{since}..HEAD");
        let out = ws
            .runner
            .run("git", &["diff", "--name-only", &range, "--"], &ws.root)?;
        if !out.success() {
            return Err(GraphError::Command(callisto_model::CommandError::Io {
                program: "git".to_string(),
                message: crate::apply::redact_git_stderr(&out.stderr),
            }));
        }
        let files: Vec<String> = out
            .stdout_trimmed()
            .lines()
            .map(|l| l.trim_matches('"').to_string())
            .collect();
        loaded
            .into_iter()
            .filter(|cs| files.iter().any(|f| cs.path.ends_with(f)))
            .collect()
    } else {
        loaded
    };

    for cs in &target_changesets {
        if cs.changeset.entries.is_empty() {
            diagnostics.push(callisto_model::Diagnostic {
                code: callisto_model::DiagnosticCode::EmptyChangeset,
                severity: callisto_model::DiagnosticSeverity::Error,
                message: format!("Changeset `{}` is empty", cs.path.display()),
                package: None,
                path: Some(cs.path.clone()),
                governed_by: None,
                escalated_by: None,
            });
        }

        if !cs.changeset.entries.is_empty() && cs.changeset.summary.trim().is_empty() {
            diagnostics.push(callisto_model::Diagnostic {
                code: callisto_model::DiagnosticCode::EmptySummary,
                severity: callisto_model::DiagnosticSeverity::Error,
                message: format!(
                    "Changeset `{}` has entries but an empty summary",
                    cs.path.display()
                ),
                package: None,
                path: Some(cs.path.clone()),
                governed_by: None,
                escalated_by: None,
            });
        }

        for entry in &cs.changeset.entries {
            match callisto_model::PackageId::parse(&entry.name) {
                Ok(id) => {
                    if !ws.graph.packages().any(|p| p.id.matches(&id)) {
                        diagnostics.push(callisto_model::Diagnostic {
                            code: callisto_model::DiagnosticCode::UnknownPackage,
                            severity: callisto_model::DiagnosticSeverity::Error,
                            message: format!(
                                "Changeset `{}` references unknown package `{}`",
                                cs.path.display(),
                                entry.name
                            ),
                            package: Some(id),
                            path: Some(cs.path.clone()),
                            governed_by: None,
                            escalated_by: None,
                        });
                    }
                }
                Err(_) => {
                    diagnostics.push(callisto_model::Diagnostic {
                        code: callisto_model::DiagnosticCode::InvalidPackageName,
                        severity: callisto_model::DiagnosticSeverity::Error,
                        message: format!(
                            "Changeset `{}` contains invalid package name `{}`",
                            cs.path.display(),
                            entry.name
                        ),
                        package: None,
                        path: Some(cs.path.clone()),
                        governed_by: None,
                        escalated_by: None,
                    });
                }
            }
        }
    }

    escalate(&mut diagnostics, opts.strict, opts.strict_graph);

    let is_valid = diagnostics
        .iter()
        .all(|d| d.severity != callisto_model::DiagnosticSeverity::Error);
    Ok(ValidateReport {
        schema_version: SCHEMA_VERSION,
        ok: is_valid,
        diagnostics,
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use callisto_model::{CommandError, CommandOutput, CommandRunner};

    use super::*;
    use crate::locate::IgnoreWalkLocator;
    use crate::Workspace;

    /// A `CommandRunner` that fails every call, echoing a stderr containing
    /// an authenticated GitHub remote URL -- the realistic shape a `git
    /// diff --cached` failure could surface in CI (e.g. a shallow clone or a
    /// detached-HEAD checkout with no cached index to diff against).
    struct LeakyGitRunner;

    impl CommandRunner for LeakyGitRunner {
        fn run(
            &self,
            _program: &str,
            _args: &[&str],
            _cwd: &Path,
        ) -> Result<CommandOutput, CommandError> {
            Ok(CommandOutput {
                exit_code: Some(128),
                stdout: String::new(),
                stderr: "fatal: unable to access 'https://x-access-token:ghs_leaked_secret@github.com/org/repo.git/': The requested URL returned error: 403".to_string(),
            })
        }
    }

    /// `validate --staged`'s `git diff --cached` failure must not leak an
    /// authenticated remote URL's credential into the resulting `GraphError`.
    #[test]
    fn staged_diff_failure_redacts_credential_from_error() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let root = dir.path();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = []\nresolver = \"2\"\n",
        )
        .unwrap();
        std::fs::write(root.join("callisto.toml"), "").unwrap();

        let runner = LeakyGitRunner;
        let locator = IgnoreWalkLocator::new(root);
        let ws = Workspace::load(root.to_path_buf(), &locator, &runner)
            .expect("empty workspace should load");

        let opts = ValidateOptions {
            staged: true,
            ..Default::default()
        };
        let err = validate(&ws, &opts).expect_err("git diff failure must surface as an Err");

        let rendered = format!("{err}");
        assert!(
            !rendered.contains("ghs_leaked_secret"),
            "credential must not survive redaction, got: {rendered}"
        );
        assert!(rendered.contains("[REDACTED]"), "got: {rendered}");
    }
}

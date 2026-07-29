use callisto_model::{CommandRunner, CreatedTag, PublishPlan, TagReport, SCHEMA_VERSION};

use crate::error::GraphError;
use crate::resolver::DependencyResolver;
use crate::Workspace;

#[derive(Clone, Debug, Default)]
pub struct TagOptions {
    pub dry_run: bool,
    pub floating_major: bool,
}

pub fn create_tags<R: CommandRunner, D: DependencyResolver>(
    ws: &Workspace<'_, R, D>,
    plan: &PublishPlan,
) -> Result<TagReport, GraphError> {
    create_tags_with_options(ws, plan, &TagOptions::default())
}

pub fn create_tags_with_options<R: CommandRunner, D: DependencyResolver>(
    ws: &Workspace<'_, R, D>,
    plan: &PublishPlan,
    opts: &TagOptions,
) -> Result<TagReport, GraphError> {
    let mut tags = Vec::new();

    for release in &plan.releases {
        let tag_str = release.tag_name.as_str();

        if opts.dry_run {
            if opts.floating_major {
                let tmpl = ws.tags.template(&release.package);
                if let Some(v_str) = tmpl.extract_version_str(release.tag_name.as_str()) {
                    let grammar = ws
                        .graph
                        .packages()
                        .find(|p| p.id == release.package)
                        .and_then(|p| p.version_grammar().ok())
                        .unwrap_or(callisto_model::VersionGrammar::SemVer);
                    if let Ok(ver) = callisto_model::Version::parse(v_str, grammar) {
                        if let Some(major_tag) = tmpl.render_floating_major(&ver) {
                            tags.push(CreatedTag {
                                package: release.package.clone(),
                                tag_name: major_tag,
                                sha: release.sha.clone(),
                            });
                        }
                    }
                }
            }
            tags.push(CreatedTag {
                package: release.package.clone(),
                tag_name: release.tag_name.clone(),
                sha: release.sha.clone(),
            });
            continue;
        }

        let output = ws
            .runner
            .run("git", &["tag", "--list", tag_str], &ws.root)?;

        let already_existed = output.success() && !output.stdout_trimmed().is_empty();

        if !already_existed {
            let msg = format!("Release {}", tag_str);
            let sha_str = release.sha.as_str();
            let create_out = ws.runner.run(
                "git",
                &["tag", "-a", tag_str, "-m", &msg, sha_str],
                &ws.root,
            )?;
            if !create_out.success() {
                return Err(GraphError::Command(callisto_model::CommandError::Io {
                    program: "git".to_string(),
                    message: create_out.stderr,
                }));
            }
        }

        if opts.floating_major {
            let tmpl = ws.tags.template(&release.package);
            let ver_str = tmpl.extract_version_str(release.tag_name.as_str());
            let grammar = ws
                .graph
                .packages()
                .find(|p| p.id == release.package)
                .and_then(|p| p.version_grammar().ok())
                .unwrap_or(callisto_model::VersionGrammar::SemVer);

            if let Some(v_str) = ver_str {
                if let Ok(ver) = callisto_model::Version::parse(v_str, grammar) {
                    if let Some(major_tag) = tmpl.render_floating_major(&ver) {
                        let sha_str = release.sha.as_str();
                        let major_out = ws.runner.run(
                            "git",
                            &["tag", "-f", major_tag.as_str(), sha_str],
                            &ws.root,
                        )?;
                        if major_out.success() {
                            tags.push(CreatedTag {
                                package: release.package.clone(),
                                tag_name: major_tag,
                                sha: release.sha.clone(),
                            });
                        }
                    }
                }
            }
        }

        tags.push(CreatedTag {
            package: release.package.clone(),
            tag_name: release.tag_name.clone(),
            sha: release.sha.clone(),
        });
    }

    Ok(TagReport {
        schema_version: SCHEMA_VERSION,
        created_tags: tags,
        diagnostics: Vec::new(),
    })
}

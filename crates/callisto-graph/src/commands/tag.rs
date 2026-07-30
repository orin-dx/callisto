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

    let repo = callisto_vcs::GitRepository::discover(&ws.root)?;

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

        let existing = repo.list_tags(Some(tag_str))?;
        let already_existed = !existing.is_empty();

        if !already_existed {
            let msg = format!("Release {}", tag_str);
            repo.create_tag(tag_str, &release.sha, Some(&msg))?;
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
                        repo.create_floating_major(major_tag.as_str(), &release.sha)?;
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
    }

    Ok(TagReport {
        schema_version: SCHEMA_VERSION,
        created_tags: tags,
        diagnostics: Vec::new(),
    })
}

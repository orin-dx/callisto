use callisto_model::{
    ApplyPermit, CommandRunner, CreatedTag, PublishPlan, TagReport, SCHEMA_VERSION,
};
use callisto_vcs::{GitAccess, GitDataSource};

use crate::error::GraphError;
use crate::resolver::DependencyResolver;
use crate::Workspace;

#[derive(Clone, Debug, Default)]
pub struct TagOptions {
    pub floating_major: bool,
}

pub fn create_tags<R: CommandRunner, D: DependencyResolver>(
    ws: &Workspace<'_, R, D>,
    plan: &PublishPlan,
    permit: Option<&ApplyPermit>,
) -> Result<TagReport, GraphError> {
    create_tags_with_options(ws, plan, &TagOptions::default(), permit)
}

/// Creates one git tag per release in `plan`.
///
/// `permit` is the dry-run gate: `None` reports the tags that *would* be
/// created without touching a single ref, `Some` creates them. It replaces the
/// former `TagOptions::dry_run` bool so that the preview branch and the write
/// branch cannot disagree -- the write calls simply do not typecheck without a
/// permit in hand.
pub fn create_tags_with_options<R: CommandRunner, D: DependencyResolver>(
    ws: &Workspace<'_, R, D>,
    plan: &PublishPlan,
    opts: &TagOptions,
    permit: Option<&ApplyPermit>,
) -> Result<TagReport, GraphError> {
    let mut tags = Vec::new();

    // `GitAccess` never hard-fails on a missing gix discovery: `wasm32` has
    // no gix at all (excluded from that target's dependency set), and even
    // natively a failed discovery just means every op below runs through
    // the `CommandRunner` fallback instead. Its write operations
    // (`create_tag`/`create_floating_major`) additionally guarantee a
    // discovered repo's result is authoritative and never masked by a
    // shell retry -- see `GitAccess`'s doc comment.
    let git = GitAccess::discover(&ws.root, ws.runner);

    for release in &plan.releases {
        let tag_str = release.tag_name.as_str();

        let Some(permit) = permit else {
            if opts.floating_major {
                let tmpl = ws.tags()?.template(&release.package);
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
        };

        let already_existed = !git.list_tags(Some(tag_str))?.is_empty();

        if !already_existed {
            let msg = format!("Release {}", tag_str);
            git.create_tag(tag_str, &release.sha, Some(&msg), permit)?;
        }

        if opts.floating_major {
            let tmpl = ws.tags()?.template(&release.package);
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
                        git.create_floating_major(major_tag.as_str(), &release.sha, permit)?;
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

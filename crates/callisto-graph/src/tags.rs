use std::collections::BTreeMap;
use std::path::Path;

use callisto_model::{
    select_last_tag, CommandRunner, CommitSha, Diagnostic, LastTag, LastTagSelection, PackageId,
    TagTemplate, VersionGrammar,
};

use crate::config::ResolvedConfig;
use crate::error::GraphError;
use crate::resolver::DependencyResolver;

pub fn last_tag_for<R: CommandRunner>(
    _runner: &R,
    root: &Path,
    template: &TagTemplate,
    grammar: VersionGrammar,
) -> Result<LastTagSelection, GraphError> {
    let glob = template.glob();
    let repo = callisto_vcs::GitRepository::discover(root)?;
    let tags = repo.list_tags(Some(&glob))?;
    let lines: Vec<String> = tags.into_iter().map(|t| t.0).collect();

    let line_strs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
    select_last_tag(template, grammar, line_strs).map_err(GraphError::from)
}

pub struct TagIndex {
    last: BTreeMap<PackageId, Option<LastTag>>,
    templates: BTreeMap<PackageId, TagTemplate>,
    pre_cursor: BTreeMap<PackageId, Option<CommitSha>>,
    pub diagnostics: Vec<Diagnostic>,
}

impl TagIndex {
    pub fn build<R: CommandRunner, D: DependencyResolver>(
        runner: &R,
        root: &Path,
        graph: &D,
        _cfg: &ResolvedConfig,
    ) -> Result<Self, GraphError> {
        let mut last = BTreeMap::new();
        let mut templates = BTreeMap::new();
        let mut pre_cursor = BTreeMap::new();
        let diagnostics = Vec::new();

        for pkg in graph.packages() {
            let tmpl = TagTemplate::parse(&format!("{}@{{version}}", pkg.id.display_name()))?;
            let sel = last_tag_for(runner, root, &tmpl, pkg.version_grammar()?)?;
            last.insert(pkg.id.clone(), sel.chosen);
            templates.insert(pkg.id.clone(), tmpl);
            pre_cursor.insert(pkg.id.clone(), None);
        }

        Ok(TagIndex {
            last,
            templates,
            pre_cursor,
            diagnostics,
        })
    }

    pub fn last_tag(&self, id: &PackageId) -> Option<&LastTag> {
        self.last.get(id).and_then(|opt| opt.as_ref())
    }

    pub fn template(&self, id: &PackageId) -> &TagTemplate {
        &self.templates[id]
    }

    pub fn pre_cursor(&self, id: &PackageId) -> Option<&CommitSha> {
        self.pre_cursor.get(id).and_then(|opt| opt.as_ref())
    }
}

---
callisto-graph: patch
---

Fix `changed_since_last_tag` walking the whole repo instead of a package's own paths. It called `commits_since(Some(&tag), &[])` with an empty pathspec, so any commit anywhere in the workspace since the package's tag short-circuited `Ok(true)` before the path-scoped `git diff --quiet <tag> -- <paths>` check a few lines below ever ran. In an active monorepo this made the function return `true` for nearly every package on nearly every call, since some other package almost always committed since -- `status`'s `changed_since_last_tag` field was effectively meaningless. `commits_since` is now scoped to `package_paths(pkg)`, identically to the diff fallback; an empty scoped result still falls through to the `git diff --quiet` check, which alone catches uncommitted working-tree changes.

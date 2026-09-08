---
callisto-vcs: minor
callisto-graph: minor
---

**Delete unused abstractions with zero real callers; share tag-glob compilation**

- `GitVcsProvider` trait and its `GitRepository` impl (`callisto-vcs`) -- fully superseded by the already-polymorphic `GitDataSource` trait (`GitRepository`/`ShellGit`/`GitAccess`, used generically throughout `callisto-graph`); `GitVcsProvider` had exactly one impl and zero callers anywhere in the workspace.
- `last_tag_for` and its `select_from_tags` helper (`callisto-graph`) -- re-exported from the crate root "for API compatibility" but the only callers were `last_tag_for`'s own unit tests; `TagIndex::build` already resolves tags via `select_from_tags_cached`.
- Added `callisto_vcs::compile_tag_glob` -- the one real, shared implementation of "compile a tag glob and map a compile failure to `VcsError::InvalidGlob`", now used by both `GitDataSource` backends and by `callisto-graph`'s tag matching instead of three independent copies.

No behavior change for any real caller.

---
callisto-cli: minor
---

**Git runs through the system `git` binary only**

The in-process gitoxide backend is gone; every Git read and write already had to work through the `git` binary the CLI requires. Commit inference keeps a commit that touched a package even when a later merge discarded its change (`git log --full-history`), matching the removed backend. An annotated release tag made with no Git identity configured takes the target commit's committer as its tagger.

Breaking:
- `callisto_model::vcs` (was `callisto-vcs`): removed `GitRepository`, `ShellGit`, the `GitDataSource` trait and the revwalk visit counter. `GitAccess::discover(root, runner)` is now `GitAccess::new(root, runner)` with inherent methods.
- `callisto_model::vcs`: `commits_since` with an unresolvable `since` ref returns `VcsError::RefNotFound` (was `VcsError::Git` from the shell path).

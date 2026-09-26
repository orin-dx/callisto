# 5. System git only

Status: Accepted (implemented in #145)

## Context

- Callisto reads commits, tags and staged changes, and creates and pushes tags.
- From 2026-08-03 `callisto-vcs` had two backends behind `GitDataSource`: native gix (`GitRepository`) and a shell fallback (`ShellGit`), with reads retrying through the shell on any native error (commit bb2a69cc4).
- The CLI already required a `git` binary: trust observation, staged changes and every fallback went through it (#145).

## Decision

Every Git command in the `callisto` binary runs the user's `git` through `CommandRunner`. `callisto_model::vcs` has one Git access type, `GitAccess`, and defines no backend trait; `GitAccess` implements callisto-model's `CommitWalker` for severity inference (`crates/callisto-vcs/src/lib.rs`). Git operations use the user's git config and identity, except release tags: with no identity set, the target commit's committer becomes the tagger, and `TagSignPolicy::ForceUnsigned` adds `--no-sign` (`crates/callisto-vcs/src/access.rs`). Workspace-root discovery looks for a `.git` entry on disk (`crates/callisto-graph/src/locate/root.rs`).

## Options considered

- **gix as the primary backend with a shell fallback (two backends)** — rejected in #145: "Two backends meant two semantics to keep in step (gix ignored TagSignPolicy) and 106 extra crates." The changeset adds: "every Git read and write already had to work through the `git` binary the CLI requires."
- **gix only** — not considered in #145. Reason not recorded.

## Consequences

- History walks use `git log --no-merges --full-history`, matching the gix commit sets; plain `git log` history simplification drops commits whose change a later merge discarded.
- Callisto depends on the installed git version and parses its output.
- Annotated tags work on runners with no git identity: the tagger falls back to the target commit's committer (#145).
- The managed release-PR action is outside this decision: it writes its commit and branch refs through the GitHub API (`.github/actions/callisto-action/scripts/create-or-update-release-pr.sh`).
- Today `release-pr commit-plan` reads staged bytes from the worktree, not the index (docs/projects/ROAD-TO-V1.md, v1 fix plan §1).

## Enforcement

- `callisto_model::vcs` has no gix dependency and one `GitAccess` type (`crates/callisto-model/src/vcs/access.rs`).
- Nothing bans gix in `deny.toml`; reintroducing it needs a new ADR.

## Revisit when

- A required Git operation is impossible or unacceptably slow through the binary, measured against the #145 numbers.
- Callisto must run where no `git` binary exists.

## Sources

- PR #145 (7ad333311): commit body, PR body, `.changeset/calm-foxes-pounce.md`
- Commit bb2a69cc4 (two-backend `GitDataSource` introduced)
- docs/00-design.md §15 `callisto-vcs` entry, describing the pre-#145 two-backend design (`git show 7ad333311^:docs/00-design.md`)

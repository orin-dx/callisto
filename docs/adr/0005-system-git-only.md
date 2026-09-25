# 5. System git only

Status: Accepted

## Context

- Callisto reads commits, tags and staged changes, and creates and pushes tags.
- From 2026-08-03 `callisto-vcs` had two backends behind `GitDataSource`: native gix (`GitRepository`) and a shell fallback (`ShellGit`), with reads retrying through the shell on any native error (commit bb2a69cc4).
- The CLI already required a `git` binary: trust observation, staged changes and every fallback went through it (#145).
- gix could not serve the moon WASM extension anyway: its object reads use mmap, which WASI rejects with ENOSYS (Track 0 spike, ffc922d44; see ADR 6).

## Decision

Every Git read and write shells out to the user's `git` through `CommandRunner`. `callisto-vcs` has one type, `GitAccess`. Callisto sees exactly the repository, config and identity that Git does.

## Options considered

- **gix as the primary backend with a shell fallback (two backends)** — rejected in #145: "Two backends meant two semantics to keep in step (gix ignored TagSignPolicy) and 106 extra crates." The changeset adds: "every Git read and write already had to work through the `git` binary the CLI requires."
- **gix only** — not viable: the shell path was already required for trust observation and staged changes (#145), and gix object reads fail under WASI (ffc922d44).

## Consequences

- History walks use `git log --no-merges --full-history`, matching the gix commit sets; plain `git log` history simplification drops commits whose change a later merge discarded.
- Callisto depends on the installed git version and parses its output.
- Annotated tags work on runners with no git identity: the tagger falls back to the target commit's committer (#145).

## Enforcement

- `callisto-vcs` has no gix dependency and one `GitAccess` type (`crates/callisto-vcs/src/access.rs`).
- Nothing bans gix in `deny.toml`; reintroducing it needs a new ADR.

## Revisit when

- A required Git operation is impossible or unacceptably slow through the binary, measured against the #145 numbers.
- Callisto must run where no `git` binary exists.

## Sources

- PR #145 (7ad333311): commit body, PR body, `.changeset/calm-foxes-pounce.md`
- Commit bb2a69cc4 (two-backend `GitDataSource` introduced)
- Commit ffc922d44 (Track 0 WASI spike)
- docs/00-design.md §15, `callisto-vcs` entry (`git show 11038b11b^:docs/00-design.md`)

<h1 align="center">Callisto Architecture</h1>

<p align="center">Crate map, data flow, and the invariants every change must preserve.</p>

---

## Invariants

1. **Safe Rust only.** `unsafe_code = "forbid"` workspace-wide.
2. **License boundary.** `callisto-model` is MIT and must never depend on an FSL-1.1-MIT crate. Every other crate is FSL-1.1-MIT. Check with `grep -H "^license" crates/*/Cargo.toml`.
3. **Format-preserving manifest edits.** `Cargo.toml` goes through `toml_edit`'s CST; `package.json` is fingerprinted for indent style and line endings before a `serde_json` (`preserve_order`) round trip. No regex or line-based edits to manifests.
4. **One file-write primitive, capability-gated.** `callisto_model::atomic::atomic_write` (`NamedTempFile` in the target's own directory, `fsync`, `persist` via `fs::rename`, then `fsync` the parent and grandparent directories) is the only way file content is written; deletions and directory creation use `std::fs`. It takes `&ApplyPermit`, a token with a private field that only `ApplyPermit::granted_unless_dry_run(dry_run)` can construct, returning `None` on a dry run. A write path that forgets to check `--dry-run` has no permit to pass and fails to compile.
5. **System Git only.** All VCS reads and writes shell out to the user's `git` binary (`callisto_model::vcs`), so Callisto sees exactly the repository, config, and identity Git itself does. No embedded Git implementation.
6. **User-facing errors are diagnosable.** Every error surfaced to a user derives `miette::Diagnostic` with a stable code and, where the fix isn't obvious from the message, `help` text. A wrapper around another crate's error (for example `GraphError::Config`) is `#[diagnostic(transparent)]` and declares no code of its own, so the inner code reaches the user. Full list: [`docs/errors.md`](docs/errors.md).

Design decisions, with the options they rejected and why: [`docs/adr/README.md`](docs/adr/README.md).

## Crate map

```
Layer 1 (leaf)     callisto-model
Layer 2 (I/O)      callisto-manifests
Layer 3 (engine)   callisto-graph
Layer 4 (surface)  callisto-cli
Dev-only           callisto-fixtures
```

| Crate | License | Depends on | Purpose |
| :--- | :--- | :--- | :--- |
| [`callisto-model`](crates/callisto-model) | MIT | none | Domain primitives (`PackageId`, `Version`, `Severity`, `Changeset`, `PreState`), `atomic_write`, `ApplyPermit`, the `CommandRunner`/`DependencyResolver` trait seams; `format`: `.changeset/*.md` and `pre.json` parser/writer; `vcs`: `GitAccess`, every Git operation as a subprocess through `CommandRunner` |
| [`callisto-manifests`](crates/callisto-manifests) | FSL-1.1-MIT | model | `Manifest` trait; format-preserving Cargo/npm/PyPI manifest editors |
| [`callisto-graph`](crates/callisto-graph) | FSL-1.1-MIT | model, manifests | Dependency graph, cascade engine, version planning, release execution; `conventional`: Conventional Commit parsing and bump inference; `changelog`: Markdown changelog rendering |
| [`callisto-cli`](crates/callisto-cli) | FSL-1.1-MIT | model, manifests, graph | `clap` CLI surface, `miette` diagnostic rendering |
| [`callisto-fixtures`](crates/callisto-fixtures) | FSL-1.1-MIT | model (dev-only) | Multi-ecosystem test corpus and in-memory test doubles |

Dependencies only point down the layers (`cli` → `graph` → `manifests` → `model`). Nothing in Layer 1–3 depends on `callisto-cli`.

## Data flow

```
Workspace::load
  config::load -> config::resolve                       callisto.toml -> ResolvedConfig
  ManifestWalkResolver::build                           discover (ProjectLocator), read manifests (Manifest),
                                                        assign ids, attach platform packages, apply per-package config
  GroupTable::resolve                                   bind [[fixed-group]] / [[linked-group]] members
plan_version -> VersionPlan (in memory)
  aggregate                                             changesets + opt-in commit inference -> severity per package
  run_cascade                                           dependents (runtime, build, optional, peer; never dev), groups, pre mode
                                                        -> target version per package, with its reasons
version --emit-decision                                 writes the release decision, before apply
apply_version_plan (needs ApplyPermit)                  CST edits -> atomic_write, changelogs, git staging
release                                                 decision -> intent -> envelope -> execute_release -> receipt
  per operation                                         observe provider -> adopt, or perform and confirm
```

Cycles are detected with `petgraph::algo::tarjan_scc`. Git access goes through `GitAccess` (`callisto_model::vcs`).

Depth:

- [`docs/architecture/identity.md`](docs/architecture/identity.md): package ids, bare-name matching, config rule specificity, workspace loading, platform packages.
- [`docs/architecture/versioning.md`](docs/architecture/versioning.md): plan and apply, idempotent re-apply after a crash.
- [`docs/architecture/release.md`](docs/architecture/release.md): release terms, the two routes, run envelope, observation and proof tokens, the transition table, verification tiers.

## Extension seams

Four traits decouple the engine from platform I/O (`callisto-model::exec`, `callisto-model::discovery`, `callisto-manifests`, `callisto-graph::locate`):

- `ProjectLocator` — enumerates workspace project roots.
- `CommandRunner` — runs a subprocess. `run_with_timeout`'s default implementation ignores the timeout; the CLI's runner enforces it, so a hung publish command cannot block forever.
- `Manifest` — per-ecosystem read/write. Mutations only touch the in-memory CST; nothing reaches disk until `persist(&ApplyPermit)`, so one open manifest can take several mutations before one write.
- `DependencyResolver` — supplies graph nodes and edges; `ManifestWalkResolver` is the only current implementation.

## Not covered here

- Release lifecycle, CI plan/build/execute, recovery: [`docs/releasing.md`](docs/releasing.md).
- Registry authentication: [`docs/publishing.md`](docs/publishing.md).
- `callisto.toml` keys: [`docs/config.md`](docs/config.md).
- Diagnostic codes: [`docs/errors.md`](docs/errors.md).
- Behavior, including `callisto matrix`: [`docs/specs/`](docs/specs/INDEX.md).

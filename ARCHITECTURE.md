<h1 align="center">Callisto Architecture</h1>

<p align="center">Crate map, data flow, and the invariants every change must preserve.</p>

---

## Invariants

1. **Safe Rust only.** `unsafe_code = "forbid"` workspace-wide.
2. **License boundary.** `callisto-model`, `callisto-format`, `callisto-vcs` are MIT and must never depend on an FSL-1.1-MIT crate. Every other crate is FSL-1.1-MIT. Check with `grep -H "^license" crates/*/Cargo.toml`.
3. **Format-preserving manifest edits.** `Cargo.toml` goes through `toml_edit`'s CST; `package.json` is fingerprinted for indent style and line endings before a `serde_json` (`preserve_order`) round trip. No regex or line-based edits to manifests.
4. **One disk-write primitive, capability-gated.** `callisto_model::atomic::atomic_write` (`NamedTempFile` in the target's own directory, `fsync`, `persist` via `fs::rename`, then `fsync` the parent and grandparent directories) is the only way anything in the workspace touches disk. It takes `&ApplyPermit`, a token with a private field that only `ApplyPermit::granted_unless_dry_run(dry_run)` can construct, returning `None` on a dry run. A write path that forgets to check `--dry-run` has no permit to pass and fails to compile — this replaced an earlier convention-based check that had already been forgotten twice (`pre enter`/`pre exit`, `init`).
5. **System Git only.** All VCS reads and writes shell out to the user's `git` binary (`callisto-vcs`), so Callisto sees exactly the repository, config, and identity Git itself does. No embedded Git implementation: gix was a second backend with diverging semantics and 106 extra crates.
6. **User-facing errors are diagnosable.** Every error surfaced to a user derives `miette::Diagnostic` with a stable code and, where the fix isn't obvious from the message, `help` text. Full list: [`docs/errors.md`](docs/errors.md).

## Design decisions

Decisions a contributor might reasonably reverse. Each names what was rejected and why.

- **Changesets file format.** `.changeset/*.md` is byte-compatible with `@changesets/cli`, so adopting or leaving callisto is one commit. Commit inference is opt-in per package (`release-trigger = "auto"`), not the default.
- **Ecosystem tools publish and own auth.** `cargo`, `npm`/`pnpm`, `twine` and `gh` publish with whatever credentials they see. Callisto has no credential pre-flight: it duplicated each tool's auth rules and drifted from them.
- **Merging the release PR is the only approval.** No GitHub Environment reviewer gate: it blocked automated releases and reviewed nothing the PR had not. Protect the default branch instead; registry credentials exist only in the `execute` job.
- **The committed decision is the release authority.** `version --emit-decision` writes `.callisto/release-decision.json` into the release PR; after merge, `release plan` verifies against it and never re-derives, so what reviewers approved is what ships.
- **Reruns adopt landed effects.** Each run observes every provider and adopts effects that already landed. No execution state is persisted; it added recovery commands and failure modes of its own. A receipt is written only when everything succeeded.
- **Platform packages release with their owner.** An `os`+`cpu` package in exactly one package's `optionalDependencies` takes the owner's version, has no tag of its own and publishes first. No `[[fixed-group]]` entry needed.
- **One build.** No cargo features in shipped crates: a feature-gated binary differed from the tested one. Behaviour is chosen at runtime (`release-trigger`).
- **proto, not a moon extension.** moon users run the CLI installed through proto. The WASM extension could not read Git objects under WASI and pulled in wasmtime (445 crates).

## Crate map

```
Layer 1 (leaf)     callisto-model  callisto-format  callisto-conventional  callisto-changelog
Layer 2 (I/O)      callisto-manifests  callisto-vcs
Layer 3 (engine)   callisto-graph
Layer 4 (surface)  callisto-cli
Dev-only           callisto-fixtures
```

| Crate | License | Depends on | Purpose |
| :--- | :--- | :--- | :--- |
| [`callisto-model`](crates/callisto-model) | MIT | none | Domain primitives (`PackageId`, `Version`, `Severity`, `Changeset`, `PreState`), `atomic_write`, `ApplyPermit`, the `CommandRunner`/`DependencyResolver` trait seams |
| [`callisto-format`](crates/callisto-format) | MIT | model | `.changeset/*.md` and `pre.json` parser/writer |
| [`callisto-vcs`](crates/callisto-vcs) | MIT | model | `GitAccess` — every Git operation as a subprocess through `CommandRunner` |
| [`callisto-conventional`](crates/callisto-conventional) | FSL-1.1-MIT | model | Conventional Commit parsing and bump-severity classification |
| [`callisto-changelog`](crates/callisto-changelog) | FSL-1.1-MIT | model | Markdown changelog rendering |
| [`callisto-manifests`](crates/callisto-manifests) | FSL-1.1-MIT | model | `Manifest` trait; format-preserving Cargo/npm/PyPI manifest editors |
| [`callisto-graph`](crates/callisto-graph) | FSL-1.1-MIT | model, vcs, manifests, format, changelog, conventional | Dependency graph, cascade engine, version planning, release execution |
| [`callisto-cli`](crates/callisto-cli) | FSL-1.1-MIT | all of the above | `clap` CLI surface, `miette` diagnostic rendering |
| [`callisto-fixtures`](crates/callisto-fixtures) | FSL-1.1-MIT | model (dev-only) | Multi-ecosystem test corpus and in-memory test doubles |

Dependencies only point down the layers (`cli` → `graph` → {`manifests`, `vcs`, `format`, `conventional`, `changelog`} → `model`). Nothing in Layer 1–3 depends on `callisto-cli`.

## Data flow

```
discover projects (ProjectLocator, ignore-aware walk)
  -> read manifests (Manifest trait: Cargo.toml / package.json / pyproject.toml)
     read VCS state (GitAccess: commits since, tags, staged changes)
  -> build the dependency graph (ManifestWalkResolver)
     detect cycles (petgraph::algo::tarjan_scc) -> miette diagnostic on a cycle
  -> aggregate changesets + Conventional Commits into per-package severities
  -> cascade: propagate a bump along Runtime/Build/Optional edges to dependents;
     Dev-only edges never force a version bump
  -> version plan: target version per package, with reason (changeset, fixed
     group, linked group, cascade, or pre-release policy)
  -> render unified diffs (preview) or persist:
     CST rewrite (toml_edit / serde_json) -> atomic_write (needs ApplyPermit)
  -> release: for each package whose current version has no tag yet,
     registry publish -> git tag -> GitHub release
```

`callisto version --emit-decision <file>` writes the exact version plan (package, target version, inclusion reason) alongside the manifest/changelog edits. A merged release PR's commit carrying that file is later the sole authority `callisto release plan --from-release-commit` verifies against — CI never re-derives cascade or group policy at that boundary, only confirms the committed diff matches what `version` already decided. See [`docs/releasing.md`](docs/releasing.md) for the full release lifecycle.

## Version groups

- `[[fixed-group]]` — members bump in lock-step to the max severity computed across the group.
- `[[linked-group]]` — members share severity, keep independent base versions.

Key reference: [`docs/config.md`](docs/config.md).

## Extension seams

Four traits decouple the engine from platform I/O (`callisto-model::exec`, `callisto-model::discovery`, `callisto-manifests`, `callisto-graph::locate`):

- `ProjectLocator` — enumerates workspace project roots.
- `CommandRunner` — runs a subprocess; `run_with_timeout` exists separately so a hung publish command doesn't block forever (the default `run` has no timeout).
- `Manifest` — per-ecosystem read/write. Mutations only touch the in-memory CST; nothing reaches disk until `persist(&ApplyPermit)`, so one open manifest can take several mutations before one write.
- `DependencyResolver` — supplies graph nodes and edges; `ManifestWalkResolver` is the only current implementation.

## Native target matrix (`callisto matrix`)

Auto-discovers napi-rs (`napi.targets`) and Maturin (`[tool.maturin].targets`) platform targets, plus `engines.node` / `requires-python` runtime constraints, straight from manifests — no hand-maintained CI matrix YAML. Java and .NET native-target discovery are not implemented. `callisto matrix [--package <name>]`, `--format json` via the global flag.

## Not covered here

- Release lifecycle, CI plan/build/execute, recovery: [`docs/releasing.md`](docs/releasing.md).
- Registry authentication: [`docs/publishing.md`](docs/publishing.md).
- `callisto.toml` keys: [`docs/config.md`](docs/config.md).
- Diagnostic codes: [`docs/errors.md`](docs/errors.md).

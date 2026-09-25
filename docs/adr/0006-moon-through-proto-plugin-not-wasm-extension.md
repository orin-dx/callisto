# 6. moon integrates through a proto plugin, not a WASM extension

Status: Accepted

## Context

- Callisto started moon-first. docs/02-library-vs-moon-decision.md chose "Option C": a moon-free core, with a moon WASM extension (`callisto-moon`) as the single reference integration next to the CLI. The core had to build for `wasm32-wasip1` and pass fixtures under wasmtime.
- The Track 0 spike (ffc922d44, 2026-08-04) tested gix inside a WASI guest. `gix-ref` worked, but `gix-odb` and `gix-pack` read objects through mmap, which WASI rejects with ENOSYS. Verdict NO-GO: the extension had to run `git` on the host through moon's `warpgate_pdk::exec_command`, like the CLI. The blocker "will not self-resolve" without WASI mmap or a gix-odb read fallback.
- moon's project-graph edges carry no version requirement, so moon could never drive the cascade; it could only locate projects and cross-check edges (docs/02 change 1 and 2).

## Decision

Callisto ships one native binary (crates.io, GitHub release archives, the `setup-callisto` action). For proto and moon users, a proto TOML plugin (`proto/callisto.toml`) installs that binary from the GitHub release assets, and moon tasks call it like any tool. There is no moon WASM extension, no WASM build and no moon-specific code in the core.

## Options considered

- **moon WASM extension (`callisto-moon`)** — removed in #142. What it duplicated or forced:
  - A second command surface: `execute_extension` dispatched the CLI's subcommands inside WASM (docs/00-design.md §11).
  - Moon-only seams in the core: `MoonProjectLocator`, `ProjectLocator::declared_edges`, `DeclaredEdge`, the `GraphEdgeDisagreement` cross-check, `IdentityResolver` (E155, E156), `--strict-graph`, the cli `wrapper` feature, and wasm32-gated fallbacks in `callisto-vcs`.
  - A second release artifact (`callisto-moon.wasm`) and installer (`setup-callisto-wasm`).
  - wasmtime in the dependency tree through `moon_pdk_test_utils → extism → wasmtime`. `deny.toml` carried 18 advisory ignores for it. A new wasmtime advisory blocked CI on every open PR (fc3db408d).
- **moon-first core (Option A)** — rejected in docs/02: knope's cascade is trapped in its binary and Nx needed a v21 rewrite after coordination logic leaked into per-ecosystem plugins.
- **Hybrid WASM extension: native `gix-ref` plus host exec for objects** — rejected by the spike: "adds complexity for minimal gain".

## Consequences

- #142: Cargo.lock 810 → 365 packages; `cargo deny` needs no advisory ignores.
- moon workspaces get the same binary as everyone else; project discovery is callisto's own walk (`IgnoreWalkLocator`). The proto plugin covers macOS arm64 and Linux x86_64 (glibc, musl) only.
- The proto plugin cannot verify checksums until releases publish `.sha256` assets (#142 follow-up).

## Enforcement

- None automated. No crate, CI job or release slot targets `wasm32-wasip1`.

## Revisit when

- moon offers something a CLI task cannot get (for example project-graph data with version requirements) and the owner wants it.
- WASI gains mmap, or gix-odb gains a read fallback. Git access is shell-only anyway (ADR 5), so this alone is not enough.

## Sources

- PR #142 (2c649a147; f8598d19e, f87aa1f4d, 5988c10cd, 1b0e7e1da), `.changeset/moon-proto-plugin.md`
- Commit ffc922d44 (Track 0 spike)
- docs/02-library-vs-moon-decision.md and docs/00-design.md §0.1, §10, §11 (`git show 11038b11b^:docs/<file>`)
- Commit fc3db408d (#38, wasmtime advisory blocking CI); `git show f8598d19e^:deny.toml`

---
callisto-cli: minor
---

Install callisto with proto: `proto/callisto.toml` downloads the release binary for macOS arm64 and Linux x86_64 (glibc or musl). moon tasks run the installed CLI.

Breaking:
- Removed the moon extension (`callisto-moon` crate) and the `callisto-moon.wasm` release asset. Use the proto plugin and run `callisto` from moon tasks.
- Removed the moon project-graph cross-check: `ProjectLocator::declared_edges`, `DeclaredEdge`, `DeclaredEdgeKind`, `DiagnosticCode::GraphEdgeDisagreement` and `LocateError::{MoonUnavailable, MoonOutputParse, IncompatibleMoonVersion}`. With it go `--strict-graph` on `status`, `version` and `snapshot`, `StrictFlag::StrictGraph`, and the `strict_graph` fields of `StatusOptions`/`VersionOptions`; `escalate` takes only `strict`.
- Removed `IdentityResolver` and its errors `GraphError::UnsupportedIdentityEcosystem` (E155) and `GraphError::PackageIdentifierParse` (E156); the `callisto-cli` `wrapper` feature is gone.

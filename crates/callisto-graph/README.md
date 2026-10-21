# callisto-graph

Dependency graph, cascade engine, changeset/commit aggregation, config resolution, and release execution for callisto. Includes Conventional Commit inference (`conventional`, formerly `callisto-conventional`) and changelog rendering (`changelog`, formerly `callisto-changelog`). The workspace's engine crate — most CLI commands are thin wrappers over this crate.

This crate is an internal part of the `callisto-cli` release tool. It is published so the CLI can be installed from crates.io, versions in lockstep with the CLI and carries no API stability guarantee: any release may change it.

License: FSL-1.1-MIT. See [ARCHITECTURE.md](../../ARCHITECTURE.md) for the full crate map and [the root README](../../README.md) for the CLI.

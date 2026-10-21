# callisto-manifests

Per-ecosystem manifest read/write for callisto: the `Manifest` trait plus format-preserving Cargo, npm, and PyPI editors (`toml_edit`, fingerprinted `serde_json`).

This crate is an internal part of the `callisto-cli` release tool. It is published so the CLI can be installed from crates.io, versions in lockstep with the CLI and carries no API stability guarantee: any release may change it.

License: FSL-1.1-MIT. See [ARCHITECTURE.md](../../ARCHITECTURE.md) for the full crate map and [the root README](../../README.md) for the CLI.

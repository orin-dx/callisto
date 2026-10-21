# callisto-model

Shared types and traits for callisto: package identity, versions, manifests, dependency specs, and the versioned JSON report contract. Also holds `atomic_write`, the `ApplyPermit` capability token, the changeset and `pre.json` formats (`format`, formerly `callisto-format`) and Git access through the git CLI (`vcs`, formerly `callisto-vcs`).

This crate is an internal part of the `callisto-cli` release tool. It is published so the CLI can be installed from crates.io, versions in lockstep with the CLI and carries no API stability guarantee: any release may change it.

License: MIT. See [ARCHITECTURE.md](../../ARCHITECTURE.md) for the full crate map and [the root README](../../README.md) for the CLI.

---
callisto-cli: minor
---

A `[registries.<name>]` entry for a custom registry must now set `kind` to `cargo`, `npm` or `pypi`; without it Callisto fails with E321 instead of silently reading the registry as npm. The built-in `cratesIo`, `npm` and `pypi` entries may leave it out and keep their own kind, so `[registries.cratesIo]` with only a `confirm-timeout` no longer turns the crates.io registry into an npm one.

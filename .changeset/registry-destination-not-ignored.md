---
callisto-cli: patch
---

A `url` on a `cargo` registry in `callisto.toml` is now rejected with E254. Callisto never routed cargo to it, so a release went to the registry in `.cargo/config.toml` or crates.io while the configuration said otherwise. Publishing an npm package with yarn when `callisto.toml` sets an npm registry `url` now stops with E168 before yarn runs, because `yarn npm publish` has no `--registry` and would have published to yarn's own registry; set `npmPublishRegistry` in `.yarnrc.yml` instead.

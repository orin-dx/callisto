---
callisto-cli: minor
---

**Publish to a private Cargo registry.** Name a `kind = "cargo"` registry in `publish-to` and Callisto publishes and checks that crate there instead of on crates.io.

- Why it matters: crates for an internal registry can now be released with the same flow as every other package.
- How to adopt it: declare `[registries.<name>]` with `kind = "cargo"`, define its URL in `.cargo/config.toml`, and set `publish-to = ["<name>"]` on the package, and give the release job `CARGO_REGISTRIES_<NAME>_TOKEN` (the name upper-cased, `-` as `_`). Callisto runs `cargo publish --registry <name>` and `cargo info --registry <name>`.
- Limits: a `url` on a Cargo registry is still rejected, because Cargo takes the URL from `.cargo/config.toml`. A name that is not a `kind = "cargo"` registry fails with E330.

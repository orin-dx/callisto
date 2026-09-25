# 0011. License split

## Context

Core parsing and model crates are useful to other tools; the engine is the product.

## Decision

`callisto-model`, `callisto-format` and `callisto-vcs` are MIT. All other crates are FSL-1.1-MIT, which becomes MIT two years after each release.

## Consequences

- MIT crates never depend on FSL crates.
- Check with `grep -H "^license" crates/*/Cargo.toml`.

# 0009. Distribution through proto, not a moon extension

## Context

The moon WASM extension could not read Git objects under WASI and duplicated the CLI surface.

## Decision

moon users install the native `callisto` CLI through the proto plugin (`proto/callisto.toml`). There is no moon extension crate.

## Consequences

- One binary serves every build system.
- moon tasks call `callisto` like any other tool.

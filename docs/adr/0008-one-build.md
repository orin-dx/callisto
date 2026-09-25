# 0008. One build

## Context

Cargo features split the shipped binary from the one CI tested, which hid bugs in the default build.

## Decision

`callisto-cli`, `callisto-graph` and `callisto-manifests` have no cargo features. The only feature, `callisto-model/test-util`, is dev-only.

## Consequences

- CI tests the binary users run.
- Optional behaviour is config (e.g. `release-trigger`), not compile-time.

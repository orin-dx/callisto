# 0010. Format-preserving, atomic manifest writes

## Context

Release tools that regex-edit manifests lose comments and ordering and can leave half-written files on interruption.

## Decision

Manifests are edited through a CST (`toml_edit`; JSON with key order and fingerprinted indentation). Every file write goes through `atomic_write`, which needs an `ApplyPermit` that a dry run cannot create.

## Consequences

- Diffs touch only version strings.
- A write path that ignores `--dry-run` does not compile.

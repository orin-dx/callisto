# 0007. Platform packages release with their owner

## Context

napi and esbuild-style packages ship one wrapper plus N `os`+`cpu` packages. The wrapper's `optionalDependencies` break if a platform version was never published.

## Decision

An `os`+`cpu` package listed in exactly one package's `optionalDependencies` belongs to that owner: same version, no own tag, pins kept in sync, published before the owner.

## Consequences

- No `[[fixed-group]]` entry is needed for platform packages.
- Changesets name the owner, not a platform package.

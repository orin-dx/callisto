# 0003. Ecosystem tools publish and own authentication

## Context

Registries, auth schemes and publish rules differ per ecosystem and change often.

## Decision

Callisto publishes by running `cargo`, `npm`/`pnpm`, `twine` and `gh`. It never reads or checks registry credentials.

## Consequences

- Whatever auth the tool sees in its environment is what's used.
- Auth failures come from the tool itself; the fix is to correct auth and rerun (see ADR 0006).

# 0012. Code never cites specs

## Context

Spec IDs, criterion IDs and section numbers in code went stale whenever specs were rewritten or deleted.

## Decision

Comments, test names and messages describe behaviour. They never cite spec, criterion, section, plan or track identifiers.

## Consequences

- Specs can be rewritten without touching code.
- Traceability comes from tests named after the behaviour they check.

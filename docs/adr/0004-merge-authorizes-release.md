# 0004. Merging the release PR authorizes the release

## Context

A second approval step (GitHub Environment reviewer) blocked automated releases and added no review the PR itself did not already get.

## Decision

Merging a verified managed release PR to the default branch is the only approval. The release workflow has no Environment or reviewer gate.

## Consequences

- Protect the default branch with required reviews instead.
- Registry credentials are scoped to the `execute` job only; plan and build jobs never see them.

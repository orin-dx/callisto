# 0005. The committed version decision is the release authority

## Context

Re-deriving versions in CI after merge can disagree with what reviewers approved in the release PR.

## Decision

`callisto version --emit-decision` writes `.callisto/release-decision.json` into the release PR. After merge, `release plan` verifies the merged commit against it and never re-derives cascade or group policy.

## Consequences

- What reviewers saw in the PR is what gets released.
- The workflow's own revision and the release-source commit are separate, both recorded.

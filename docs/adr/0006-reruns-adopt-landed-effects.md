# 0006. Reruns adopt landed effects

## Context

A release can fail midway, after some registries, tags or GitHub releases already exist. Persisting execution state added recovery commands and failure modes of its own.

## Decision

Every run observes each provider first and adopts any effect that already landed, then performs the rest. No execution state is persisted. A receipt is written only when every operation succeeded.

## Consequences

- "Re-run failed jobs" or a plain rerun completes a partial release.
- Releasing an older merged source uses a new run with current workflow code and an explicit source SHA.

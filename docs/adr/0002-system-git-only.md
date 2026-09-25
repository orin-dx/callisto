# 0002. System git only

## Context

An embedded Git library (gix) had to track repository layouts, config, credentials and identity that `git` already handles, and it doubled build time and binary size.

## Decision

Every Git read and write runs the user's `git` binary through `callisto-vcs`.

## Consequences

- Callisto sees the same repository, config, hooks and identity as the user.
- `git` must be on `PATH`; history walks use `git log --no-merges --full-history`.

---
callisto-model: patch
callisto-manifests: patch
callisto-vcs: patch
callisto-graph: patch
callisto-format: patch
callisto-changelog: patch
callisto-cli: patch
---

# Polyglot Engine Re-Architecture, Native Gitoxide Ref Engine, and Diagnostic Cards (since v0.3.0)

## Architectural Enhancements
- **Polyglot Resilience Engine (`callisto-graph`, `callisto-model`)**: Added `RegistryClient`, `RateLimitPolicy`, and `TimeProvider` traits. Built `PublishOrchestrator` supporting HTTP 429 server TTL parsing, 600-second auto-pause handling, and immediate auth fail-fast on 401/403 credentials errors.
- **Native Gitoxide Subprocess Elimination (`callisto-vcs`)**: Replaced external CLI `git` subprocess spawns with native `gix` (Gitoxide) ref operations. Enforced `PreviousValue::MustNotExist` transaction constraints to prevent accidental tag reference overwrites on collisions.
- **CST Manifest Hygiene (`callisto-manifests`)**: Unified TOML and JSON manifest editing under `ManifestCstEditor`. Preserved `.decor()` comments across scalar values, inline tables, and block table dependencies in `Cargo.toml`.
- **Rich Terminal Diagnostics (`miette`)**: Standardized all workspace error enums on `miette::Diagnostic` with explicit canonical error codes (`E001`–`E130`) and actionable help cards.

## Core Bug Fixes
- **Fixpoint Engine Severity Escalation**: Refactored `union_fixed` and `union_linked` to track `changed: bool` across iterations, ensuring transitive severities propagate fully when existing keys escalate from `Patch` to `Major`.
- **Toposort Self-Loop Cycle Resolution**: Fixed 1-node self-loop cycle extraction (`A -> A`) in `toposort.rs` and ensured cycle paths follow directed outgoing graph edges.
- **Cascade Metadata Preservation**: Prevented stale `reasons` and `governed_by` metadata overwrites during severity raising in `cascade.rs`.
- **Boundary Line Ending Hygiene**: Stripped UTF-8 BOM (`\u{FEFF}`) headers and normalized CRLF (`\r\n`) line endings across changeset parsers and stdin readers.
- **Default PR Branch & Emoji Hardening**: Set default release PR branch to `callisto/version-packages` and purged AI emoji filler.

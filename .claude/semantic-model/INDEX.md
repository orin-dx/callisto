# Semantic Model — Load Index

Read this first. Load only the files your task requires — never load all files at once.
The goal is progressive loading: ~400 tokens here, then ~1500 tokens per topic file loaded.

## Task → Files to Load

| You are working on... | Load these files |
|---|---|
| `apply.rs`, version writing, idempotency | `version-flow.md`, `error-taxonomy.md` |
| `walk.rs`, config resolution, package overrides | `config-resolution.md`, `core-identity.md` |
| `identity.rs`, PackageId, ecosystem matching | `core-identity.md` |
| `error.rs`, adding or using error variants | `error-taxonomy.md` |
| `callisto release`, release execute/recover, receipts, provider observation | `release-lifecycle.md`, `error-taxonomy.md` |
| Changing behavior in an area | `docs/specs/<area>.json` |
| Starting fresh, no task assigned | `docs/projects/ROAD-TO-V1.md` |

Specs in `docs/specs/` are the contract; these files describe how the code implements it.

## File Map

| File | Covers |
|---|---|
| `core-identity.md` | PackageId variants, Ecosystem, bare vs prefixed, matches() semantics, rule-specificity decision |
| `version-flow.md` | VersionPlan, PlannedBump, apply_version_plan, idempotency guard |
| `config-resolution.md` | ResolvedConfig, [[package]] vs [[package-set]], PackagePattern, specificity ordering |
| `error-taxonomy.md` | All GraphError + ConfigError variants, E-codes, when to emit each |
| `release-lifecycle.md` | Local `callisto release` route, run envelope, evidence-carrying provider observation, operation transition table, release E-codes, wire versions |

## What Does NOT Live Here

- Source code — read it directly from the crate
- Spec@1 artifacts — live in `docs/specs/`
- Plans and open work — live in `docs/projects/`
- User preferences — lives in the memory directory
- Prose design docs — live in `docs/` (treat as human reference, not invariant source)

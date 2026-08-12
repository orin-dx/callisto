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
| publish pipeline, registry, PublishPlan | `ARCHITECTURE.md §9` (not yet in semantic-model) |
| Implementing a specific track | `.claude/specs/<track>.json` only |
| Starting fresh, no task assigned | `.claude/plans/ACTIVE.md` |

Do NOT load semantic-model files when implementing a track unless the spec explicitly
references them. The spec@1 is the authority; these files are the reference layer beneath it.

## File Map

| File | Covers |
|---|---|
| `core-identity.md` | PackageId variants, Ecosystem, bare vs prefixed, matches() semantics, Track E decision |
| `version-flow.md` | VersionPlan, PlannedBump, apply_version_plan, idempotency guard, Track B design |
| `config-resolution.md` | ResolvedConfig, [[package]] vs [[package-set]], PackagePattern, specificity ordering |
| `error-taxonomy.md` | All GraphError + ConfigError variants, E-codes, when to emit each |

## What Does NOT Live Here

- Source code — read it directly from the crate
- Spec@1 artifacts — live in `.claude/specs/`
- Active task tracking — lives in `.claude/plans/ACTIVE.md`
- User preferences — lives in the memory directory
- Prose design docs — live in `docs/` (treat as human reference, not invariant source)

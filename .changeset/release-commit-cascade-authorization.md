---
callisto-graph: patch
callisto-model: patch
---

**Fix: a merged release commit with a fixed/linked-group cascade bump was rejected as an unreviewed release**

`release plan --from-release-commit` re-derives the release roster from a merged commit's diff, authorizing only packages a consumed changeset names directly. It had no notion of fixed/linked groups, whose members always converge to one shared target version the moment any member bumps (Track 1) -- so any release touching a group (this repository's own `workspace` fixed group among them) failed with `E124: release intent no longer matches the current workspace snapshot`, blocking every downstream plan/build/execute step.

`derive_release_commit_decision` now recognizes a package's version change as authorized when it's a member of a group triggered by a directly changeset-named sibling, and records the correct `FixedGroup`/`LinkedGroup` reason for it. A new `E155` diagnostic (`ReleaseCommitCascadeDivergent`) fails closed if a triggered group's members land on different target versions instead of converging, so a tampered or malformed commit is still rejected rather than silently trusted. Dependency-driven `Cascade` and pre-release-policy bumps remain unsupported and continue to fail closed, unchanged from before.

Also fixes `ReleaseInclusionReason`'s `FixedGroup`/`LinkedGroup`/`Cascade`/`PreReleasePolicy` variants serializing their fields (`group_id`, `edge_kind`, `policy_id`) as snake_case instead of the camelCase the type's own `rename_all` already promised for its `kind` tag -- the same `rename_all`-does-not-rename-variant-fields gap fixed for `ReleasePrActionV1` in `fix-release-pr-action-camel-case.md`.

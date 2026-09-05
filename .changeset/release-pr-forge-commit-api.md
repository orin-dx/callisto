---
callisto-model: minor
---

**Update the managed release PR to use GitHub's forge commit API instead of a local Git push**

`ReleasePrSnapshotV1`, `ReleasePrActionV1`, and `ReleasePrDecisionV1` are replaced by schema-version-2 equivalents: `ReleasePrSnapshotV2`, `ReleasePrActionV2`, and `ReleasePrDecisionV2`. `ReleasePrPullRequestV1`'s `workflow_delta_from_base: bool` is replaced by `ReleasePrPullRequestV2`'s `head_commit: CommitSha`. `ReleasePrActionV2` drops the `Supersede` variant entirely -- there is no replacement, and no runtime fallback to it -- in favor of `Noop`, `Create`, and `Update` variants that name a deterministic staging branch. A new `ReleasePrCommitPlanV1` type builds the typed `createCommitOnBranch` payload from a Git index diff, refusing (with new error codes E149-E154) any `.github/workflows/*` path, non-regular-file Git modes, renames/copies/type-changes, and oversized payloads.

This is consumer-facing: on a public GitHub repository, the built-in `GITHUB_TOKEN` cannot write `.github/workflows/*` through either the Git push protocol or `createCommitOnBranch`'s own file changes, which previously forced a SHA-suffixed replacement branch and PR (visible churn) whenever a workflow file drifted on the base branch. The new approach never writes that path at all, so the replacement branch behavior is gone and updates land on one stable branch. A branch already replaced under the old behavior remains a valid, ordinary managed branch and keeps being updated in place.

Removing the v1 `ReleasePr*` types and the `Supersede` variant is a breaking change for any library consumer of `callisto-model`; this ships as a minor bump since the crate is pre-1.0.

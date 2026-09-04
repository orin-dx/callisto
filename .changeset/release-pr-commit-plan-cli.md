---
callisto-cli: minor
---

**Add `callisto release-pr commit-plan`**

A new read-only `callisto release-pr commit-plan --base-commit <sha> --message <msg> [--out <file>]` subcommand renders a `ReleasePrCommitPlanV1` as JSON from the current Git index diff against `<sha>`. It is the building block the release action now uses to stage a release-PR update through GitHub's `createCommitOnBranch` commit API instead of a local `git push`, so the built-in `GITHUB_TOKEN` never needs `.github/workflows/*` write permission on any ref. This removes the SHA-suffixed-replacement-branch churn the prior local-push fallback caused whenever a workflow file changed on the base branch.

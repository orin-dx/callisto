---
callisto-vcs: minor
---

**Add `ShellGit::staged_changes_since` observation**

`ShellGit` gains a `staged_changes_since(base) -> Vec<StagedChangeV1>` observation (parsed from `git diff --cached --raw`, reading worktree bytes for each entry), exposed through `GitAccess`. This is the credential-free input the release action uses to build a `ReleasePrCommitPlanV1` for the managed release PR, part of moving that update off a local `git push` and onto GitHub's forge commit API so the built-in token never needs `.github/workflows/*` write permission.

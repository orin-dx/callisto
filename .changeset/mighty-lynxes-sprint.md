---
callisto-cli: patch
---

**Generated release workflows carry any valid branch name.** `callisto init` no longer truncates a default branch such as `release/next` to `next`, and no longer breaks the workflow YAML for branch names containing `,`, `#`, `]` or similar.

- **Why it matters:** the push trigger and the action `branch` input now name the real default branch, so version PRs target the right base.
- **Retrying the workflow:** when init skips the workflow (for example, the release tag cannot be resolved), `callisto init --workflow` now adds just the workflow to an initialized workspace and leaves `callisto.toml` untouched. Before, it refused with E190.
- **Limits:** generated branch names are always double-quoted in the workflow; a leading `+` or `!` is escaped for the trigger filter.

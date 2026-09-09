---
callisto-graph: patch
---

`render_pr_body_from_plan` looked up each bump's matching `ChangelogWrite` with `plan.changelog_writes.iter().find(...)` inside the loop over `plan.bumps` -- an O(bumps * changelog_writes) scan for every PR body render. Builds a `HashMap<&PackageId, &ChangelogWrite>` once before the loop instead, mirroring the `pkg_map` precedent already used by `plan_version` in `version.rs` for the identical anti-pattern.

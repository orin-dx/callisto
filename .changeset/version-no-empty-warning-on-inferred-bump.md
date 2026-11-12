---
callisto-cli: patch
---

`callisto version` no longer says "No pending changesets found in workspace" when it has just planned a bump inferred from commits.

- With `release-trigger = "auto"` and a `feat:` commit after the last tag but no changeset file, `version` printed the plan (for example `cargo/a` 0.1.0 to 0.2.0) and then warned there were no changesets, which contradicted the plan beside it. It now prints only the plan.
- `version --strict` turned that warning into an error and exited 1 on a valid inferred plan. It now exits 0.
- The JSON `diagnostics` array no longer carries the `empty-changeset` diagnostic when the bump was inferred from commits.
- When there is nothing to release (no changesets and no inferred bumps), the warning still appears, and `--strict` still fails on it. `status` is unchanged.
- You do not need to do anything, unless a script relied on that warning (or the `empty-changeset` diagnostic) to detect inferred bumps. Check the plan output instead.

---
callisto-cli: patch
---

**Retrying `version` finishes when its emitted decision is inside the workspace.** A run that wrote `--emit-decision` or `--emit-preview` into `.changeset` and was then interrupted no longer refuses its own retry with `E340`.

- A hand edit to that file after the interruption is still refused.

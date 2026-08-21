---
callisto-graph: minor
---

# One changelog renderer, everywhere

- PR bodies now render through the same code `CHANGELOG.md` uses, instead of a separate hand-rolled Markdown path.
- Inference-driven bumps and fixed-group new-member joins now get real changelog entries instead of a placeholder.
- A bump with multiple contributing causes in one run (changeset + cascade, say) now lists all of them, not just one.

---
callisto-cli: patch
---

Python dependencies pinned in `[project.optional-dependencies]` extras are now followed: bumping a package releases the packages that pin it in an extra and rewrites the pin, instead of leaving stale published metadata.

---
callisto-graph: patch
---

**PyPI publish stops forcing `--skip-existing` against private indexes**

Twine 5+ refuses to run at all with that flag against any repository other than the public warehouses (`upload.pypi.org` / `test.pypi.org`), so it's now included only when the target is one of those.

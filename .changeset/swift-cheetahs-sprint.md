---
callisto-cli: patch
---

**Equivalent package names are rejected as duplicates.** Two PyPI projects such as `My_Pkg` and `my-pkg`, or two crates such as `foo-bar` and `foo_bar`, can no longer both load and publish under one registry name.

- Why it matters: a registry treats these as one project, so the second upload and its tag could never be valid.
- Selectors follow the same rule: `pypi/my-pkg` now selects a package declared as `My_Pkg`, and `[[package]]` rules match the same way.

**Action:** if a workspace fails with E100 after upgrading, rename one of the two packages.

**Limits:** glob patterns such as `pypi/my-*` still match the spelling in the manifest.

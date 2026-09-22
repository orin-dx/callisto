---
callisto-cli: patch
---

**Typed diagnostic for GitHub's workflow guard on tag push**

When GitHub refuses a release tag push because a GitHub App token (`GITHUB_TOKEN`) cannot push a commit whose `.github/workflows/` differs from every branch tip, `callisto release` now reports `E180` naming the tag and its target commit, with the remedy: push that tag with a non-App credential, then re-run recovery. It previously surfaced as a generic `E164` with raw git stderr. Other push failures are unchanged.

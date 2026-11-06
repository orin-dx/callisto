---
callisto-cli: patch
---

The generated matrix release workflow now checks out the release commit in its `plan`, `build-python` and `execute` jobs. A branch checkout left HEAD attached, so each of those commands stopped with E170 and the release could not run. An existing workflow needs `ref: ${{ github.sha }}` on those three checkouts.

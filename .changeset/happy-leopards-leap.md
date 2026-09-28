---
callisto-cli: patch
---

`callisto release-pr verify` accepts `callisto release-pr decide --format json` output as-is again, so the release action can open and update the release PR. 0.9.0 failed with E281.

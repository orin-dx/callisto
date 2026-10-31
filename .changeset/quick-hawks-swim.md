---
callisto-cli: minor
---

The release action no longer has a pending_pr_label input. Merged releases are now found with callisto release pending, so nothing reads or sets that label. A workflow that still passes the input gets an unused-input warning from GitHub; delete the line.

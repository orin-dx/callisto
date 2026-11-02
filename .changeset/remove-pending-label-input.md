---
callisto-cli: minor
---

The release action's `pending_pr_label` input is removed; merged releases are found with `callisto release pending`, so nothing sets or reads that label. Delete the input from your workflow if you pass it.

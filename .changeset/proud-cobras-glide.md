---
callisto-cli: patch
---

**`version --dry-run` refuses where `version` would**

A dry run now predicts the real run when an interrupted run left a checkpoint.

- Why it matters: a dry run used to report a plan even when the checkpoint came from another workspace, or `HEAD`, the tags or Callisto had changed, where `version` refuses with E340.
- Limits: a dry run still does not compare the files the interrupted run wrote, since it writes nothing.

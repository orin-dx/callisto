---
callisto-cli: patch
---

Every add/version/schema/pre/release-pr error now carries its own code and help text instead of a generic message; release-pr decide/verify report the exact invalid-branch or duplicate-pull-request code instead of a plain string when reading a snapshot or decision from a file, inline JSON, or stdin.

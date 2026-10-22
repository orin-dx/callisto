---
callisto-cli: patch
---

When a release stops because a registry or forge could not be observed (E176) or a publish is not yet visible (E157), the error names the operation in words, such as "tag of cargo/demo 0.10.0" or "registry publish to internal of npm/demo 1.0.0", and ends with the most recent failed command and the last 20 lines or 2 KiB of its stderr, with credentials redacted.

---
callisto-cli: patch
---

When a release stops because a registry or forge could not be observed (E176) or a publish is not yet visible (E157), the error now ends with the most recent failed command and the last 20 lines or 2 KiB of its stderr, with credentials redacted.

---
callisto-cli: patch
---

`callisto release plan --artifact-repository` without `--orchestration-revision` no longer fails as a usage error: a source with `[release]` fails with E231, and one without it ignores the flag.

---
callisto-cli: patch
---

callisto status now reads a changeset's package names the same way callisto version does. A package named by a non-primary manifest's own name resolves, a qualified name for an ecosystem the package lacks is reported as unknown, and a name that two packages claim is ambiguous, so status --check no longer passes a changeset that version will not consume.

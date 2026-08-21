---
callisto-graph: patch
---

**Cascade correctness: peer-escalation severity and cross-ecosystem rewrite keys**

- A dependent package no longer gets over-escalated to a major/minor bump when the upstream change was actually a patch.
- Publishing a package that exists in both Cargo and npm no longer crashes during version bumps with a "dependency not found" error.

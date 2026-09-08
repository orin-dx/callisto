---
callisto-model: patch
---

`ReleaseDecisionV1::new` now rejects a decision that claims divergent target versions for two entries tagged with the same `FixedGroup`/`LinkedGroup` inclusion reason. A fixed or linked group always converges every member to one shared target version; a decision claiming otherwise is malformed, whether freshly derived from a version plan or read back from a committed release-decision file (the digest-validating `Deserialize` impl calls `new()` internally, so this check applies to both). The check is self-contained: it reads only the decision's own entries and their self-declared group tags, with no workspace/`GroupTable` access needed.

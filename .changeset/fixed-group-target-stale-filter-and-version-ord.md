---
callisto-graph: patch
callisto-model: patch
---

**Fix `fixed_group_target` reading a stale group member's (missing) slot in `base`, corrupting the shared alignment version**

Track 1 (Fixed-group convergence) in `solve_cascade` passed the raw `GroupDef` into `groups::fixed_group_target`, which iterated every declared member -- not just the ones still present in `input.base` -- when picking the group's alignment base. A group member removed from the workspace but still declared in `callisto.toml`, if it happened to carry a release tag from before its removal, could be picked as `released[0]`; `base.get(&released[0])` then missed (the stale id has no entry in `base`) and silently fell back to the hardcoded `Version::semver(1, 0, 0)` default, corrupting the group's shared target version for every live sibling. `fixed_group_target` now takes an already-filtered `live_members` slice instead of the raw `GroupDef`, mirroring the stale-member guard the Linked-group block directly above it already applies -- a stale member can no longer occupy the alignment slot regardless of declaration order or tag history.

**Fix `ReleaseOperationId`/`ArtifactSlotId`'s `Ord` comparing a `Version` field via `render()` instead of `(grammar, raw)`**

Residual instance of the same defect class as the earlier `ReleaseOperationRole` fix (which stopped `Ord` from dropping `attestation_policy`): both hand-written `Ord` impls keyed their `version: Version` field on `version.render()` alone, dropping `version.grammar()`. The identical literal string parses under more than one grammar (`"1.2.3"` is valid both as SemVer and PEP 440), so two otherwise-identical `ReleaseOperationId`/`ArtifactSlotId` values that are `Eq`-distinct only by grammar compared `Ordering::Equal`, violating the invariant `BTreeSet`/`BTreeMap` require: `a.cmp(b) == Equal` iff `a == b`. Both now key the version field on `(grammar, raw)` via a shared `version_ord_key` helper.

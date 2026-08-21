---
callisto-graph: patch
---

# Cascade correctness: peer-escalation severity and cross-ecosystem rewrite keys

- Peer-dependency escalation now respects the actual cascading severity instead of always jumping to Major — a Patch-level upstream change no longer escalates a peer to Major just because the range fell out of coverage.
- Dependency-spec rewrite keys for a cross-ecosystem cascade are now built from the dependent's ecosystem-native identity instead of `PackageId::name()`, fixing a crash (`ManifestError::DependencyNotFound`) on an ordinary Cargo+npm co-located package.

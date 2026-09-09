---
callisto-graph: patch
---

The mapping from a package's canonical manifests to ecosystem-qualified `ReleasePackageId`s was independently reimplemented at three sites: `derive_release_decision`, `derive_release_commit_decision` (which recomputed it a second time within the same function, from values it had already derived once), and `release::derive_release_inputs` (via a differently-shaped `BTreeSet<Ecosystem>` dedup step). `derive_release_decision` and `derive_release_commit_decision` are the two ends of the same trust boundary -- one computes the release roster, the other verifies it -- so drift between their derivations was a real risk to that boundary's guarantee.

Added `release_decision::release_package_ids` as the single derivation, used by all three sites; deleted the redundant in-function recomputation in `derive_release_commit_decision`. No behavior change for any real caller.

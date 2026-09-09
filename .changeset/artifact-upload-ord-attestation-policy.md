---
callisto-model: patch
---

**Fix `ReleaseOperationId::Ord` dropping `attestation_policy` for artifact-upload operations**

`ReleaseOperationId`'s hand-written `Ord` impl folded an `ArtifactUpload` role down to just `platform`/`asset_name`, silently dropping `ArtifactSlotId::attestation_policy` (repository, workflow path, workflow commit) from the comparison -- even though it's part of `Eq`/`Hash`. Two artifact-upload operations for the same package/version/platform/asset_name but a different `attestation_policy` compared `Ord::Equal` while remaining `Eq`-distinct, violating the invariant `BTreeSet`/`BTreeMap` require. `validate_operations`'s duplicate check and `stable_kahn_order`'s prerequisite graph both key on `ReleaseOperationId`, so the second such operation's `BTreeSet` insert silently returned `false` as if it were a real duplicate, and `ReleaseIntentV1::new` wrongly rejected a legitimate, distinct release intent with `DuplicateOperation`.

`ReleaseOperationRole` now derives `PartialOrd`/`Ord` instead of `ReleaseOperationId` hand-decomposing it into a lossy sort key; every variant's field types were already `Ord`, so the derived impl automatically covers every field of every variant (including `attestation_policy`) consistently with the derived `Eq`, and stays correct if a role variant ever gains a field.

---
callisto-model: patch
---

`release.rs` and `release_pr.rs` each hand-wrote the same wire schema-version gate independently: deserialize into a private `Wire` struct, compare `wire.schema_version` against `Self::SCHEMA_VERSION`, and return a hand-written "unsupported ... schema version" `serde::de::Error::custom(...)` on mismatch. Six sites did this separately -- `ReleaseDecisionV1`, `ReleaseIntentV1` (which also inlines the same check for its nested `decision` and `snapshot` schema versions) in `release.rs`, and `ReleasePrConfigV1`, `ReleasePrSnapshotV2`, `ReleasePrDecisionV2`, `ReleasePrCommitPlanV1` in `release_pr.rs`.

Extracted `check_schema_version` as the single implementation, called from all six `Deserialize` impls. No behavior change -- each site's exact error wording is preserved verbatim (the six messages already differed from each other by type name, e.g. "unsupported release decision schema version" vs. "unsupported release PR commit plan schema version", so each is now produced by passing that same descriptive name into the shared helper rather than standardized to one wording).

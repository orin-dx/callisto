# Release wire compatibility

Which version each release document writes, which versions it reads, and what a reader does with any other. Behavior: [`docs/specs/release.json`](../specs/release.json). Release types: [`release.md`](release.md). This is a record of current behavior, not a policy; gaps are listed as gaps.

## Documents

| Document | Where it lives | Writes | Reads | Another version |
| --- | --- | --- | --- | --- |
| `ReleaseDecisionV1` | `.callisto/release-decision.json`; embedded in every intent | 3 | 1 to 3 | Versions 1 and 2 are accepted as read: they have no `consumedChangesets`, and a release commit verified against one must delete at least one changeset. Version 3 requires `consumedChangesets`; versions 1 and 2 reject it. Any other version fails to decode; reading the committed file reports E166. |
| `ReleaseInputSnapshotV1` | inside an intent only | 1 | any version when decoded alone | Checked only by the intent decoder, which accepts 1 only. |
| `ReleaseIntentV1` | `release plan --out`; the input of `release execute` and `release artifact-manifest` | 5 | 5 | Rejected before any output file is written or provider is touched: E215 from the CLI pre-check, E295 from the model decoder. |
| `ArtifactManifestV1` | `release artifact-manifest --out` | 1 | any version | No decode-time gate. A parse failure is E237; `validate_for_intent` accepts version 1 only. |
| `ReleaseRunEnvelopeV1` | inside a receipt only | 3 | 3 | Rejected at decode with an `UnsupportedSchema` error that has no diagnostic code. |
| `version` checkpoint | the commit at `refs/worktree/callisto/checkpoint/version`; its message carries `Callisto-Head`, `Callisto-Tags` and `Callisto-Version` trailers | 1 | 1 | Any change in `Callisto-Version` refuses the resume with E340, so another version never replays a checkpoint. |
| `ReleaseReceiptV1` | `release --receipt`, `release execute --receipt`; also the JSON stdout of both | 2 | 2 | Rejected at decode with an `UnsupportedSchema` error that has no diagnostic code. |

- The decision digest covers the sorted consumed changesets (version 3 only) and the sorted entries (package, target version, reasons), not the schema version, so decisions of any version with the same entries and no consumed changesets share a digest. The digest is verified on every read.
- A receipt is written only when every operation ended `published` or `alreadySatisfied`; a failed run leaves none.
- The release-PR documents (`ReleasePrSnapshotV2`, `ReleasePrDecisionV2`, `ReleasePrConfigV1`, `ReleasePrCommitPlanV1`) are separate types read and written by `release-pr decide`, `release-pr verify` and `release-pr commit-plan`, not by the release decision above. Each accepts exactly its own version. The config, snapshot and decision fail with E155; the commit plan fails with a plain message that has no code.

## Rerun with an older intent

A rerun never reuses an intent from another build. `release execute --intent` and `release artifact-manifest --intent` read the file first and fail with E215 when its version is not 5, before the run envelope is minted. The help says to run `release plan` again. A state file left by an older build is ignored, not read or migrated.

## Readers outside the model crate

- Shell and `jq` readers carry no version gate and depend on shape: the release workflow reads `.artifactSlots` from the intent, the generated build-matrix workflow reads `.operations[].id.package` and `.artifactSlots[]`, and `run-release.sh` reads the receipt's outcome paths.
- `release-pr` snapshots are built by `create-or-update-release-pr.sh` with a hard-coded `schemaVersion: 2`, while the action's `version` input can pin another binary.
- The standalone JSON stdout of `release` and `release execute` flattens the receipt and carries its schema version; the nothing-to-release report is a separate shape with version 1.

## Known gaps

- The snapshot and the artifact manifest have no decode-time version gate of their own.
- The intent decoder accepts a nested decision of any readable version; the decision keeps the version it was read with.
- 0.20 renamed the receipt's evidence keys (`peeled_commit`, `tag_name`, `byte_length`) and the preview's `short_sha` to camelCase without a version bump, so a receipt or preview written by 0.19 does not decode.
- Two readers tightened without a version bump: the artifact attestation's `sourceCommit` now equals the slot's workflow commit rather than the release source, and decision, operation and slot versions parse with the owning package's ecosystem grammar.
- The receipt's version does not track its nested types: the envelope went from 1 to 3 and the operation role vocabulary grew while the receipt stayed at 2.
- No production code decodes a receipt or an envelope from bytes. Their decode gates and the receipt schema are exercised by tests only.
- A version 1 decision can carry an `unreleasedVersion` reason, which only version 2 defines, and nothing rejects it.
- `callisto schema --type release-receipt` is the only published release schema, and its `schemaVersion` is an unconstrained integer. `callisto schema --type snapshot` is the `snapshot` command's report, not `ReleaseInputSnapshotV1`.
- The version 2 intent never existed as its own commit, and the digest transcript version is one constant shared by the decision, snapshot, intent and registry-binding digests.
- The generated consumer workflow passes the intent and manifest between jobs without a checksum or comparison step, which this repository's own release workflow has.

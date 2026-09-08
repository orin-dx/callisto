---
callisto-cli: minor
callisto-graph: patch
---

Replace release-commit verification's changeset re-derivation with persist-and-verify. `callisto version --emit-decision <path>` now writes the exact release decision (package, target version, inclusion reason) it computed, alongside the manifest and changelog edits it already writes. `callisto release plan --from-release-commit` (now requiring a companion `--decision <path>`) verifies the merged commit's actual diff against that committed decision instead of re-deriving changeset, fixed-group, linked-group, cascade, and pre-release-policy inclusion from raw git history a second time.

The prior re-derivation only understood a direct changeset-to-package match: it rejected any real release where a fixed-group cascade bumped a sibling package the deleted changeset never named, which is every release in a fixed-group workspace once more than one package versions together. Trusting the already-computed decision, tamper-checked via its own content digest, removes that entire reimplementation and the drift it was exposed to.

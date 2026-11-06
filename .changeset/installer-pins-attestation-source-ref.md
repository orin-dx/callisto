---
callisto-cli: patch
---

The `setup-callisto` action now requires the release attestation to come from `refs/heads/main`, so an asset attested from another branch or a manually dispatched run no longer verifies.

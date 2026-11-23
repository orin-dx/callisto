---
callisto-cli: patch
---

**setup-callisto verifies the Callisto binary by default**

Workflows from `callisto init` now install a verified binary in the job that holds publish credentials, without tuning.

**Breaking:** `setup-callisto` defaulted to `verification: fallback`, which installed from crates.io when verification failed. It now defaults to `require`, which fails the step instead of running an unverified binary, and no longer falls back to crates.io when the release asset is missing. Set `verification: fallback` on runners without `gh` or when installing a release that predates attestations.

- Why it matters: the job with `id-token: write` and registry credentials only runs a binary that the release workflow attested.
- How to adopt: nothing to change on GitHub-hosted runners; the failure message names the opt-out.

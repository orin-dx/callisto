---
callisto-cli: minor
---

**setup-callisto always verifies the Callisto release**

Workflows from `callisto init` install a binary whose release attestation was verified, in the job that holds publish credentials.

**Breaking:** `setup-callisto` no longer accepts the `verification` input and never falls back to installing from crates.io. It downloads the release archive, runs `gh attestation verify` on it and only then extracts it; a missing platform asset, a failed verification or a missing `gh` fails the step. Remove `verification:` from your workflow. On runners without `gh` or on other CI systems, follow `docs/install-in-ci.md`.

- Why it matters: the job with `id-token: write` and registry credentials only runs a binary that the release workflow attested.
- How to adopt: nothing to change on GitHub-hosted runners, which include `gh`.
- Limits: releases built before the first attested release carry no attestation and cannot be installed by the action.

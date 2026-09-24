# Release workflow behavior harness

This harness runs the compiled `callisto` CLI against temporary Git worktrees and fake Cargo, GitHub CLI, Git, and attestation providers. It verifies the same boundary the release workflow invokes without contacting a registry or GitHub.

Coverage includes initial publication, interruption after an attempted publish, a fresh rerun that adopts landed effects, immutable-intent validation, and the four product artifact slots. The product test verifies byte manifests, provenance verification, exact GitHub Release uploads, ignores hidden and nested unlisted files, and reruns without a duplicate asset upload.

Run it with `just release-workflow-behavior`. PR CI runs the same command in the cross-platform test job.

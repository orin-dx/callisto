---
callisto-cli: minor
---

`callisto init` now generates a release workflow for workspaces that ship `[[release.artifact]]` binaries or napi platform packages, instead of skipping them. The file is `.github/workflows/callisto-release.yml` with four jobs: `version-pr`, `plan` (`callisto release plan`), `build` (a matrix of the napi targets from `callisto matrix` plus one entry per artifact slot in the release intent) and `execute` (`napi artifacts`, then `callisto release artifact-manifest`, then `callisto release execute`). Planning runs only when the pushed commit writes `.callisto/release-decision.json`, so ordinary pushes leave `plan`, `build` and `execute` skipped.

Workspaces with maturin platform builds, or with platform packages and no `napi.targets`, still get no generated workflow. Passing `--workflow` for one of them fails with E202 and names the reason. Otherwise `init` skips the question and prints a note, which also appears in `InitReport.diagnostics` as `workflow-generation-unsupported`.

`callisto matrix --format json` now lists each cargo `[[release.artifact]]` binary as a `cargo` entry in `platformTargets`, with the slot's asset name as its `artifactName`. A package that declares both platform targets and release artifacts fails with E203.

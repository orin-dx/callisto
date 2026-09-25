# Specifications

Each spec (`spec@1`) states current behavior as testable criteria, grouped by topic prefix. No history: Git records how the behavior got here, and [`docs/adr/`](../adr/README.md) records the design decisions that still hold.

| Spec | Covers |
| --- | --- |
| [`workspace.json`](workspace.json) | Root and member discovery, package identity, per-package config rules, npm platform packages |
| [`versioning.json`](versioning.json) | Changeset format, severity aggregation, cascade, groups, prerelease, snapshot, applying versions, changelogs, the release decision file |
| [`release.json`](release.json) | `callisto release`, the CI route, decision authority, operation order, registry publish and observation, reruns, tags and GitHub releases, the managed release PR |
| [`cli.json`](cli.json) | Command surface, output and exit codes, `status`, `add`, `init`, generated workflows, `pre`, `snapshot`, `completions`, `schema` |
| [`matrix.json`](matrix.json) | `callisto matrix`: native build targets and runtime constraints for CI |

`REQ-DX-V1.md` is the open v1 requirement; delete it when v1 ships.

How the code implements these specs: [`ARCHITECTURE.md`](../../ARCHITECTURE.md) and [`docs/architecture/`](../architecture/).

When code and a spec disagree, reproduce the behavior first. Fix the spec if the intended contract changed; otherwise fix the code.

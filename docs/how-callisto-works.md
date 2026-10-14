# How Callisto works

Callisto turns changesets and native package metadata into a version plan. It then coordinates the selected registry, Git, and GitHub operations. Current scope is Cargo, npm/pnpm, and Python/PyPI.

## Where it fits

Use Callisto when contributors should declare release impact and a repo needs native dependency updates across its supported package types. The version plan makes each bump reviewable before publication.

Other tools may fit your existing workflow:

- [Changesets](https://github.com/changesets/changesets) focuses on npm package releases.
- [Knope](https://knope.tech/tutorials/releasing-multiple-packages/) offers configurable release workflows.
- [Nx Release](https://nx.dev/docs/features/manage-releases) integrates with Nx projects and tasks.
- [Release Please](https://github.com/googleapis/release-please/blob/main/docs/design.md) builds release PRs from conventional commits.

## A change through the system

Suppose a Cargo crate `service` depends on `core`. A contributor records a change to `core`:

```sh
callisto add --package cargo/core:minor --summary "Add streaming API"
callisto status
callisto version --dry-run
```

Callisto builds the version plan from the changeset, manifests, and `callisto.toml`:

- The changeset requests a minor bump for `core`.
- The Cargo dependency in `service` tells Callisto whether the new version fits its existing requirement.
  - Under the default `out-of-range` policy, a version outside that requirement can bump `service` and update its dependency declaration.
  - Cascade mode and bump severity are configurable.
- Fixed and linked groups can add packages through explicit policy.
- Unrelated npm and Python packages do not join solely because they share the repo.

After reviewing the plan, `callisto version` updates affected manifests, lockfiles, and changelogs. It stages the versioned state for a commit. `callisto release --dry-run` then previews the release operations.

## The model

| Concept | What it means today |
| --- | --- |
| Workspace package | A dependency graph node located from source and manifests. |
| Distribution | A native Cargo, npm, or PyPI identity described by a manifest. One package can expose multiple supported distributions. |
| Native dependency | A manifest relationship that can affect dependent versions and publication order. |
| Version group | An explicit fixed or linked rule in `callisto.toml`. |
| Release operation | One registry, tag, GitHub release, or asset effect with prerequisites. |

These relationships serve different purposes. For example, a Rust library embedded in a Python wheel is a build input. That alone does not create a Cargo registry dependency or make the crate and wheel share a version. See [configuration](config.md) for supported dependency and version-group rules.

## Releasing and recovery

- **Local release:** `callisto release --dry-run` previews the plan. `callisto release` runs it from a clean checkout.
  - Native publishers use their own credentials.
- **Managed CI:** The version PR commits the selected package and version decision. CI verifies it after merge.
  - Planning declares operations. Building creates artifacts. Execution receives registry credentials.
- **Execution:** Callisto checks each provider before acting.
  - It adopts an exact match, performs and confirms an absent effect, and stops on a conflict or indeterminate result.
  - A receipt records the evidence collected.

Release operations follow known dependency and platform prerequisites. They do not follow a fixed Cargo, npm, then PyPI sequence. The [release guide](releasing.md) covers the local and CI paths, retries, and recovery.

## Current support

Callisto is pre-v1. These rows describe current behavior rather than planned adapters.

| Ecosystem | Versioning | Publishing |
| --- | --- | --- |
| Cargo | Crate versions, supported dependency declarations, lockfiles | Cargo |
| npm/pnpm | Package versions, supported native dependency forms, lockfiles | Native npm tooling |
| Python/PyPI | Static PEP 621 or Poetry versions | Build backend and Twine |

| Layout | Current boundary |
| --- | --- |
| Cargo product binaries | Generated CI builds discovered Cargo binary targets. Other products need a user authored builder. |
| napi platform packages | Generated CI supports configured targets. Its workspace wide roster can include unrelated addons. |
| Co-located Cargo and Maturin | Discovery fails; no generated Maturin workflow. |
| Custom product assets | Declared artifact slots can use a manual CI builder and Callisto's execute route. |

Known release limits matter when choosing a workflow:

- A multi-registry release is not atomic.
- A tag does not yet prove that every distribution from one source package was published.
- A fresh local retry can skip unfinished work after creating a tag.
- PyPI observation does not yet prove that every intended wheel and sdist was uploaded.
- Callisto does not infer arbitrary cross-language dependencies. Go and NuGet publishers are not available.

Package owners control native lifecycle hooks. Callisto invokes supported tools without rewriting those hooks. See [publishing](publishing.md) for credentials and Python index limits, and [releasing](releasing.md) for the complete operation and recovery rules.

# Road to v1

Unshipped work only. Baseline: main `07154021c` (0.10.2). [Owner requirement](../specs/REQ-DX-V1.md), [findings](V1-FINDINGS.md), [implementation and stacked-PR execution](V1-IMPLEMENTATION.md), [task definitions](V1-SYSTEM-HARDENING.json), [scoped model and prior art](../architecture/polyglot-v1.md).

## Scope and guardrails

- Rust/Cargo, npm/pnpm and Python/PyPI now, including concrete napi-rs and Cargo/Maturin layouts. Go and C#/.NET/NuGet are architecture examples only.
- Canonical native distribution references; deliberate legacy migration; separate release ownership and version authority.
- Reuse the dependency graph and operation DAG. Native dependency, build input and version coupling do not mean the same thing.
- Native tools/CI own building, packing and authentication. No universal package manager, build scheduler, public plugin platform, service graph or cross-registry transaction.
- Keep committed release authority and provider-observed recovery with no execution journal. Only execute receives registry secrets; no GitHub Environment or separate reviewer gate in this repo.
- Each implementation PR updates affected specs, tests and docs, with one changeset per independent user-facing topic. Future behavior is not inserted into current specs before its implementation.

## Sequence

The execution portfolio holds the remaining work as bounded units, including the D01 owner-contract checkpoint. The execution guide defines plugin routing, agent write ownership, serial Cargo verification and main-targeted stacked PRs under the existing CI trigger.

### 1. Correct the model in bounded changes

- D01: confirm exact ownership, compatibility, native authority and registry-output proof contracts before architectural code; existing-contract repairs do not wait for it.
- T06 and T06-HISTORY: canonical new references and legacy/history migration across selectors, config, groups, changesets, reports, pre-state and tags. Rename, manifest additions, promotion and demotion retain the right owner's history. [ADR 12](../adr/0012-canonical-distribution-release-ownership.md) remains proposed until implementation.
- T07: supported version authority and native distribution representations. Cover co-located Cargo/Maturin with Cargo-owned dynamic Python version and inspect built metadata; do not insert a second static version or build a universal converter.
- T08: shared physical effect ownership, so one tag, draft, upload or publication referenced by several distributions is executed once. The tag already waits for every registry publish of its package, and a rerun after a failed push completes the release. Retain the committed roster and no-journal executor. A future Go source tag is publication, not a universal completion marker.

The plan is an implementation portfolio, not an approved future-behavior spec. Resolve wire/compatibility choices and update the matching area spec with implementation; do not treat architectural packets as fully gated implementer handoffs before D01.

### 2. Reconcile generated workflows

The generated build-matrix workflow builds and places napi addons only for packages in the release intent. Native lifecycle hooks remain package-owner-defined and outside Callisto's release-operation graph; Callisto does not inspect or alter them.

A native Python argv test does not imply a generated Maturin workflow, authenticated private-index observation or complete wheel-set recovery.

### 3. Verify supported workflows and onboarding

Each implementation unit adds its own regression/fault tests. T12 integrates these representative cases through production/native paths; T12-DX measures onboarding:

| Fixture | Evidence needed |
| --- | --- |
| Cargo workspace | Inherited/root versions, renamed dependencies, locked resolution and index propagation |
| npm/pnpm | Workspace ranges, aliases, paths, catalogs, peer/optional/private policy and packed metadata |
| Python | Native versions, extras/markers, prereleases, required wheel/sdist set and partial upload |
| Cargo+napi | Selected target builds, owner-defined lifecycle hooks preserved, Callisto-owned publication ordering and clean install/load |
| Cargo+Maturin | Co-located static/dynamic metadata, native version mapping and wheel metadata |
| Mixed workspace | No unrelated addon builds, foreign-name rewrites or missed affected lockfiles |
| History/retry | Correct ownership through transitions; missing push/forge/assets/files completed with original artifacts |

Retain local real-registry tests and record missing prerequisites as skips. Run `just ci` for implementation handoffs and workflow contract/policy checks for workflow changes. Measure fresh-repo first publication against the owner's ten-minute goal. Go/NuGet examples check model assumptions only; they are not new runtime gates.

## Owner decisions

- Canonical-reference compatibility, including colon form; source-owner representation and versioned wire migration.
- Authenticated private PyPI/HTML-only scope, npm channel policy, universal checksum requirements and wrapper bump severity.
- Inferred-only/manual releases: managed PR selection depends on changesets and verification requires a consumed changeset (ADR 2). Decide before broadening authorization.
- Build or drop a workspace-wide source-error-discard check; the Cargo lint is only a partial guard.
- Default tags for fixed/independent mode must be settled with canonical ownership, not a second tag-selection mechanism.
- Release-commit provenance: an unkeyed digest and decision/diff agreement do not prove which producer created the file. Decide whether existing authority is sufficient.
- Rust-cache pin in orin-dx/actions, bot-PR check policy and token rotation remain owner items.

## Other open work retained

- Audit literal-false ApplyPermit construction in release/init; enforce dry-run safety at entry boundaries.
- npm writes normalize arbitrary JSON layout; decide whether more than key order/indentation/line endings is needed.
- Preserve redacted stderr for indeterminate provider failures; never stream unredacted native-tool output.
- Publish final migration notices for retired callisto-format, callisto-vcs, callisto-conventional and callisto-changelog, naming callisto-model or callisto-graph.
- Evaluate rustc-provided target facts instead of the 18-triple table without making the core model Rust-target-specific.
- Require kind in explicit registry entries and attest napi .node outputs; define scope before adding a gate.
- Pin the floating moon minor in .prototools after checking proto compatibility.

## After 1.0 (additive)

- Effective-config explain from ConfigProvenance; remove redundant default-valued config afterward.
- .sha256 release-asset sidecars and proto verification.
- Glob fixed-group members.
- Auto-include README, LICENSE and CHANGELOG in release archives.
- Go/NuGet adapters and additional relationships only when concrete use cases and native tests justify them.

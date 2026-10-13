# Road to v1

Unshipped work only. Baseline: main `8ef9aa5a4` (#168), audited on 2026-09-26. T01 and T11 are implemented and verified locally, awaiting merge; [batch evidence](V1-VERIFICATION.md) records the scope. Proposed model work remains unimplemented, and local checks do not mark a release ready. [Owner requirement](../specs/REQ-DX-V1.md), [findings](V1-FINDINGS.md), [implementation and stacked-PR execution](V1-IMPLEMENTATION.md), [task definitions](V1-SYSTEM-HARDENING.json), [scoped model and prior art](../architecture/polyglot-v1.md).

## Scope and guardrails

- Rust/Cargo, npm/pnpm and Python/PyPI now, including concrete napi-rs and Cargo/Maturin layouts. Go and C#/.NET/NuGet are architecture examples only.
- Canonical native distribution references; deliberate legacy migration; separate release ownership and version authority.
- Reuse the dependency graph and operation DAG. Native dependency, build input and version coupling do not mean the same thing.
- Native tools/CI own building, packing and authentication. No universal package manager, build scheduler, public plugin platform, service graph or cross-registry transaction.
- Keep committed release authority and provider-observed recovery with no execution journal. Only execute receives registry secrets; no GitHub Environment or separate reviewer gate in this repo.
- Each implementation PR updates affected specs, tests and docs, with one changeset per independent user-facing topic. Future behavior is not inserted into current specs before its implementation.

## Sequence

The execution portfolio splits the themes below into 24 bounded work units, including the D01 owner-contract checkpoint. Independent release/wire, native-resolution and installer lanes can begin in parallel; generated workflows and architectural stacks follow their actual dependencies. The execution guide defines plugin routing, agent write ownership, serial Cargo verification and main-targeted stacked PRs under the existing CI trigger.

### 1. Restore the managed release path

T01 is verified locally: decide-to-verify JSON compatibility and the action's exact post-update head transition, with real binary, model and stateful shell tests. Strict payload and unrelated-change rejection remain intact. Merge that correction before the bot can refresh its release PR.

Then let the bot reconstruct the release PR from current main, verify its committed decision/source and CI, and perform the actual self-release. Do not merge stale #113 as a workaround or claim source inspection proves publication. v0.9 is the immediate release-path priority, not evidence that all v1 defects are fixed.

### 2. Close existing contract failures

- T02: wire-shaped schemas and complete real-document validation, not only top-level fields.
- T03, T03-PRE and T03-HISTORY: ecosystem-context version decoding, Python pre-state, historical inherited Cargo version authority and Git-relative paths.
- T04, T04-PNPM and T04-LOCK: manifest-local dependency names/constraints and lock refresh for every affected canonical ecosystem. Exercise pnpm workspace ranges, aliases, paths and catalogs with native packing.
- T05: interrupted versioning across refresh/staging/decision output without losing input or silently reporting success.
- T11: locally verified, awaiting merge. Installer source ownership and requested-version fidelity; a consumer mentioning callisto-cli never selects a local build.

These fix existing promises. Do not weaken retry, native-resolution or ordering contracts to excuse their failures.

### 3. Correct the model in bounded changes

- D01: confirm exact ownership, compatibility, native authority and registry-output proof contracts before architectural code; existing-contract repairs do not wait for it.
- T06 and T06-HISTORY: canonical new references and legacy/history migration across selectors, config, groups, changesets, reports, pre-state and tags. Rename, manifest additions, promotion and demotion retain the right owner's history. [ADR 12](../adr/0012-canonical-distribution-release-ownership.md) remains proposed until implementation.
- T07: supported version authority and native distribution representations. Cover co-located Cargo/Maturin with Cargo-owned dynamic Python version and inspect built metadata; do not insert a second static version or build a universal converter.
- T08 and T08-RETRY: shared physical effect ownership, publication/announcement prerequisites and retry selection after a tag exists. Retain the committed roster and no-journal executor. A future Go source tag is publication, not a universal completion marker.
- T09 and T09-ROUTES: declared registry-output inventory, Python partial-file recovery, yank/conflict semantics and private-index routing. Complete only required selected files/platforms, without guessed platform matrices or unapproved checksum/channel policy.

The plan is an implementation portfolio, not an approved future-behavior spec. Resolve wire/compatibility choices and update the matching area spec with implementation; do not treat architectural packets as fully gated implementer handoffs before D01.

### 4. Reconcile generated workflows

T10-BINS and T10-AUTH handle binary inventory, early unsupported-builder rejection and native authentication independently of the future wire model. T10 then derives build/placement rosters from the verified intent. Native lifecycle hooks remain package-owner-defined and outside Callisto's release-operation graph; Callisto does not inspect or alter them.

A native Python argv test does not imply a generated Maturin workflow, authenticated private-index observation or complete wheel-set recovery.

### 5. Verify supported workflows and onboarding

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
- GroupTable::from_groups skips resolve's conflict check; close before a production caller appears.
- Correct receipt run_attempt rationale: artifact names are already attempt-scoped; update workflow comment and policy-check message.
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

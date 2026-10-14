# V1 implementation and stacked-PR execution

Status: active execution portfolio. Baseline: `origin/main` at `07154021c` (0.10.2). [Task definitions](V1-SYSTEM-HARDENING.json), [findings](V1-FINDINGS.md), [roadmap](ROAD-TO-V1.md), [proposed model](../architecture/polyglot-v1.md).

The current-contract repairs (release-PR compatibility, installer fidelity, schemas, durable versions, native dependencies, lockfiles, interrupted versioning, binary inventory and native authentication) shipped in 0.10.0 to 0.10.2. What remains is the D01-gated model work, the effect and registry-completion stacks, intent-selected workflow outputs and the production verification pass. Architectural D01 choices, remote delivery and publication remain separate decisions.

## Architectural stack after D01

| Stack | Order | What each boundary proves |
| --- | --- | --- |
| Identity/authority | T06 canonical references → T06-HISTORY historic ownership; T07 native/Maturin authority follows T06 | One resolver across authored/generated consumers; no transferred or regressed release history; native metadata agrees with planned versions |
| Effects/retry | T08 shared physical effects → T08-RETRY tagged-candidate completion | One owner per irreversible effect; required publications gate announcements; local retry does not lose push/forge work |
| Python completion | T09 required file-set recovery → T09-ROUTES read/upload destinations | Complete declared files, not first-file presence; no changed bytes or untrusted destination on retry |
| Workflow integration | T10 intent-selected outputs | Builds follow the verified release intent; package-owner lifecycle hooks retain native behavior and remain outside Callisto's operation graph |
| Evidence | T12 production matrix → T12-DX measured onboarding | Executed/skipped/simulated evidence distinguished; first-release setup and credentials included in timing |

T06-HISTORY → T07 is a true stack: historical and native-authority changes share model/graph consumers. T08 follows both; T08-RETRY → T09 is also serialized because both change local/CLI release entry boundaries. Independent read-only review and native fixture drafting can run alongside those implementation units once their interfaces are fixed; competing edits cannot. T09-ROUTES follows T09 because both own the registry adapter. JSON task dependencies are authoritative; stack grouping is not permission to ignore a dependency.

## D01: approved directions and remaining model choices

The owner approved Cargo, npm/pnpm and Python/PyPI as current scope, architectural room for Go and .NET/NuGet, canonical-only new exact references, registry publication before shared tags/public announcements, and package-owner control of native lifecycle hooks. Remaining wire shapes and migration rules below are proposals, not hidden release gates.

| Contract | Proposed bounded choice | Approval/evidence needed |
| --- | --- | --- |
| Canonical references | The owner chose canonical slash-qualified IDs for every new exact reference; reject new bare and colon-form aliases, and read existing forms only through an explicit ownership migration boundary | Specify which historical records remain readable and how migration is surfaced; cover config, groups, changesets, pre-state and reports |
| Owner identity | Reuse a stable source-directory locator plus its explicit canonical distribution roster; do not introduce an opaque globally persistent ID | Choose a durable owner field versus deterministic derivation from the retained source snapshot; specify rename/move refusal or explicit mapping |
| Tag migration | Do not rename existing tags; retain observed legacy templates/history and select one deterministic template for new releases | Settle independent/fixed defaults with owner identity; do not add a second inference-based tag authority |
| Wire compatibility | Preserve existing string versions where context suffices; version documents whose owner/effect/output semantics change | Enumerate decision/intent/manifest/envelope/receipt readers, accepted old versions, stale decisions and old-intent rerun support; reject unsupported migration explicitly |
| Conflicting pre-state aliases | A legacy bare key may resolve to an owner that already has a canonical key in the same file | Choose behavior when both keys carry different versions; current `or_insert` keeps the canonical value and silently drops the migrated legacy value |
| Maturin | First support Cargo-owned `dynamic = ["version"]` and inherited Cargo authority with native Python mapping | Specify static co-located metadata behavior, precedence, unsupported mappings/backends and built wheel/sdist checks; do not force equal numeric strings into one grammar |
| Announcement | The owner chose required Cargo/npm/PyPI publications before a shared tag and public GitHub release; a future Go source tag can itself be a publication | Specify draft creation and upload ordering and prove all required owner outputs complete before public announcement |
| Registry file set | Declare selected outputs before build; bind exact trusted filenames/digests after build without mutating sealed intent | Choose per-file effects or an aggregate operation with explicit partial-state proof; define how build inventory is covered by the existing envelope and how local retries retain original bytes |
| Python destinations | Separate credential-free index/read and upload routing; public PyPI/TestPyPI first | Choose support versus explicit refusal for custom, authenticated and HTML-only indexes; Twine credentials do not authenticate observation automatically |
| Native lifecycle hooks | Package owners control hook scripts and any effects they initiate; Callisto preserves native behavior and does not inspect, rewrite, suppress or reject hooks | Settled owner boundary: preserve it in future publisher changes; do not require hook migration or promise that external hook effects are coordinated |

D01 records testable contracts in the proposed architecture/ADR and PR packets. Area specs remain current-behavior documents: amend them on each implementation branch with the behavior, and review/hash-bind that contract before a gated implementation handoff. ADR 12 stays proposed until owner acceptance and the implementing merge. No single approved future delta spec currently exists, so the portfolio JSON intentionally has no `linked_spec` or fabricated `spec_hash`; affected criterion mappings are not coverage of all 220 existing criteria.

## Stack mechanics

Use Git worktrees and native GitHub stacks.

- Create only authorized branches/worktrees from a committed, recorded baseline. Each worker gets a named parent revision, task packet and exclusive write set. Do not branch agents from a dirty shared checkout.
- Stacked PRs are native GitHub stacks (`gh stack link <PRs bottom to top>`); plain chained PRs targeting a parent branch get no CI because `.github/workflows/callisto-ci.yml` triggers only for PRs targeting `main`. Each PR must pass `just ci` on its own.
- Review the child delta with `rtk proxy git diff <parent>...<child>`. Also inspect the complete `main`-to-child diff and run tests on the combined stack. A passing parent is not verification of its child.
- Merge parent first. Save its reviewed tip before merge; after a squash/rebase merge, replay only child commits onto updated main, using the recorded parent tip as the exclusion boundary (for example `rtk proxy git rebase --onto origin/main <recorded-parent-tip> <child-branch>`). Inspect the resulting parent-only/full diff and rerun applicable checks before merging the child. Do not assume GitHub removes ancestor commits automatically. A required force push needs explicit approval and uses `--force-with-lease`.
- Keep stacks shallow: normally two or three open dependent PRs. Merge a verified compilation boundary before extending the next model layer; do not accumulate the full architecture as one speculative stack.
- Independent lanes are sibling PRs, not artificial ancestry. Shared-interface changes land in their common parent before parallel descendants implement adapters/fixtures against those fixed interfaces.
- Each PR includes its own regression and current spec/doc amendment. Use one changeset per independent consumer-facing topic; do not add a changeset just for internal planning or duplicate a parent's changeset in a child delta.
- Commit/PR actions remain coordinator-owned and owner-approved. Workers never push, merge, approve CI, edit release policy or publish versions.

## Plugin execution route

Use a light route for existing defects and the fuller route only when a domain contract changes. Do not make every small shell patch run the complete requirement lifecycle.

| Situation | Plugins/skills | Artifact or check |
| --- | --- | --- |
| Already verified current-contract defect | Navigator packet → Smith bounded implementation → Ranger targeted sibling/defect-family review → Sentinel scoped verification | Exact diff, regression/fault evidence, current criterion mapping and remaining checks |
| Canonical ownership, native authority or output-proof change | Scribe architecture/spec work and applicable audit/verify/gate steps → Navigator amendment → Smith → independent review/Sentinel | Owner-confirmed contract, versioned wire policy, interface inventory and raw spec hash |
| New domain gap discovered during implementation | Stop only that batch; Scribe architect/spec correction and Navigator amendment | Evidence-backed escalation; do not silently invent a second semantic model |
| New request beyond this scope | Weaver capture/connect/clarify only as needed | Traceable need; no unapproved requirement becomes a gate |
| User-visible release impact | Courier changeset | Verified topic-specific entry targeting `callisto-cli` |
| Commit and stacked PR handoff | Courier commit/PR after permission | Parent-only diff, full-stack evidence, base branch, migration risks and authorized remote action |

Plugins guide reasoning and evidence; they do not themselves provide isolated worktrees, a branch scheduler or authorization. Final synthesis, interface decisions and binding readiness stay with the coordinator. Sentinel must report pending/unverifiable evidence rather than converting a targeted test into a whole-release pass.

## Agent and model allocation

Prefer the host's inherited model unless a bounded job benefits from a deliberate smaller override. The currently exposed delegation runtime permits `gpt-6-luna`, `gpt-6-sol` and the parent frontier model; do not rely on unavailable role-card model names or promise costs/latencies.

| Work | Allocation | Boundary |
| --- | --- | --- |
| Paths, symbols, test commands and branch/CI inventory | `gpt-6-luna`, low effort, recon | Read-only deterministic inventory; no architectural verdict |
| Shell installer fixtures, link/schema checks, fixed doc edits | Small model at low/medium effort where the interface is fixed | Explicit files and expected cases; escalate failed reasoning instead of lowering verification |
| Straightforward parser/lock/auth correction | Workhorse model, medium effort; raise effort for alias/auth subtleties | One subsystem, fixed owner contract and native regression |
| Canonical identity, historical authority, DAG/proofs and partial-upload semantics | Coordinator/strong model, high effort | Own cross-crate interfaces and failure semantics; no concurrent competing models |
| Implementation once interfaces are fixed | Workhorse implementer, medium/high effort | One isolated task/worktree with no shared write set |
| Independent challenge/review | Separate workhorse reviewer, high effort for model/retry changes | Read final diff plus tests; never rely on the author's self-certification |

Start with two implementing workers plus one read-only reviewer/recon, not an agent per finding. Increase concurrency only when there are disjoint, unblocked tasks and the coordinator can inspect every result. A packet contains objective, persisted paths/hashes, owned files, exclusions, criterion/test cases, dependencies and a completion condition. Workers preserve other changes and return changed paths, commands, actual outcomes, skips and unresolved concerns.

For parallel lanes, shared specs, architecture/README/operator docs, changeset creation and the repository Cargo.lock are coordinator-owned integration surfaces. Workers return proposed amendments for those files rather than editing them concurrently; the coordinator applies them on each PR branch before its final checks. Reserve shared Rust source and integration-test files explicitly before delegation; serialize packets whose write sets overlap. A forked agent conversation is not filesystem isolation: verify its actual worktree before allowing writes.

No concurrent Cargo builds/tests locally, even across tasks. Reserve one build slot; use sequential scoped runs and reuse the shared cache. Read-only analysis, shell-only tests, review and drafting can run while that slot is occupied. Worktree/build-directory changes need approval where they change environment or resource use; they are not a reason to bypass the repository's build rule.

## Interface sibling inventory

| Shared contract | Confirmed participants | Required disposition |
| --- | --- | --- |
| `Manifest` / `ManifestIdentity` / `VersionSource` | `CargoToml`, `PackageJson`, `PyprojectToml`; `read_identity`; live base-version and historical blob readers | T03-HISTORY and T07 migrate all reached readers/editors, including writers and snapshots; preserve native parsing and format rules |
| Native edges/specs | `DependencyEntry`, `DepEdge`; Cargo/npm/Python extraction; `ManifestWalkResolver::build` in `walk.rs`; cascade/floor/snapshot; fixture `InMemoryGraph` and graph test doubles | T04/T04-PNPM migrate producers, alias-aware resolution, constructors and rewrites together; correcting cascade alone cannot recover an omitted alias edge |
| Selector ownership | `IdentityIndex`; add/status/aggregate/matrix; config rules/groups; `lib.rs::sole_owner_at` and pre-state/history readers | T06/T06-HISTORY unify exact selectors, distinguish glob syntax and migrate both tag/pre-state ownership consumers |
| `ReleaseProviderSet` | Production `ValidatedReleaseIntent` and simulator `SimWorld` | T08/T09 migrate both; fake CLI provider harness still exercises real dispatch |
| Registry adapter/evidence | `CargoRegistry`, `NpmRegistry`, `PypiRegistry`; platform npm path | T09/T09-ROUTES give every reached sibling a migrated or evidence-backed unaffected disposition |
| Immutable release wires | Decision, input snapshot, intent, artifact manifest, envelope, receipt, schemas; CLI file/stdout producers and action scripts | T02 inventory; D01 fixes compatibility; T08/T09 migrate producers/consumers, including CLI registry-only inventory acceptance/envelope sealing and local build preparation |
| Selected builds and auth | Generated simple/matrix workflows, shared action execution, build matrix/placement, policy/contract tests | T10-AUTH/T10 verify all routes, not just this repo's custom workflow; lifecycle scripts remain package-owner-controlled |

This is an inspected candidate inventory, not a promise that no new call site exists. Each implementer re-runs syntactic and responsibility-based searches on its actual base. An external lifecycle hook or unavailable native environment remains an explicit coverage gap.

## Verification at each boundary

1. Record baseline and test prerequisites; write a failing reachable regression before the correction where feasible. Check the exact parent delta and structurally identical siblings.
2. Run the scoped commands in the task JSON sequentially; use native pack/install/build evidence where semantics require it. Test negative, stale, partial and conflicting states, not only a successful fresh release.
3. Review the changed diff and evidence independently. For model/proof/retry work, use a deliberate fault or mutation targeted to the changed invariant; ordinary docs/shell checks do not need a generic mutation campaign.
4. At each merge handoff, run `rtk proxy just ci` unless an explicit evidence-backed docs-only scope justifies narrower local checks; remote required checks still apply. `just ci` includes workflow contract/policy and behavior tests plus 90% coverage, but excludes actionlint, the real-binary release-PR contract and artifact-preflight builds. Run relevant excluded checks explicitly and require their CI results. Do not describe a second policy-script run as extra coverage already provided by `just ci`.
5. After restacking, repeat applicable checks on the new combined revision. Live `just provider-contract` / fixture refresh need network and are outside ordinary CI; recorded/skipped/live evidence must remain distinct.

| Fixture family | Required evidence |
| --- | --- |
| Producer/consumer | Real decide JSON fed unchanged into verify; strict malformed/stale inputs still rejected |
| Native versions/history | Stable Python decisions and pre-state; root/inherited Cargo authority; old schemas and tamper negatives |
| Native dependencies | Promoted dual outputs; Cargo aliases/inheritance; pnpm ranges/aliases/paths/pack transforms; Python markers/extras preserved |
| Version interruption | Tracked/untracked input; late refresh/staging/decision failures; no double bump, false success or unrelated edits |
| Maturin | Co-located dynamic/static boundary; native mapping; built wheel/sdist metadata and clean installation |
| Release recovery | Tag/push/draft/upload/publication failures; full owner barrier; original trusted bytes retained |
| Registry completion | Partial Python file upload, conflicting bytes, yanked files, index lag and changed read/upload routes through production providers |
| Generated workflow | Intent-selected builds/placement, fresh native auth, unsupported builder rejection and one NAPI effect owner |
| Installation/DX | Explicit action version fidelity; complete Cargo binary inventory; fresh documented first-release timing including credentials/commits |

## Retained roadmap work: explicit disposition

- Included sibling checks: dry-run permit entry boundaries (T05/T08), alternate group-constructor conflict enforcement (T06) and redacted indeterminate provider diagnostics (T08). If a reachable fix is outside its task's file boundary, record a follow-up rather than silently omit it.
- Owner decisions, not invented merge gates: workspace-wide discarded-source-error enforcement, extra npm JSON-layout preservation, stronger release-commit producer provenance, mandatory explicit registry kind and napi output attestation, private Python/HTML support, npm channels, universal registry checksums and wrapper bump severity.
- Owner/external operations: token rotation, bot-PR check policy, Rust-cache pin in `orin-dx/actions`, final migration-notice publications for retired crates. Do not perform these as side effects of implementation agents.
- Separate small follow-up, pending compatibility check: pin the floating Moon version in `.prototools`. Evaluate target facts versus the 18-triple table without adding a Rust-specific core model requirement.
- After v1 remains additive: effective-config explain, checksum sidecars/proto verification, fixed-group globs, standard archive docs and concrete Go/NuGet adapters.

## Governing source snapshots

Raw hashes below bind this planning baseline, not a gated future spec. Recompute on the actual implementation parent. A changed source requires checking affected task coverage and amending the packet, not blindly executing a stale plan.

| Source | SHA-256 |
| --- | --- |
| `docs/specs/workspace.json` | `6cd6b2dfc3148abe273e69991793cd93817e26665bf7058f3d6af22d64263a95` |
| `docs/specs/versioning.json` | `3fa5863c70764ef48a4ccb54fea0999a84c2fa1be9491586ede99af16a087afd` |
| `docs/specs/release.json` | `5d60fa115e352c7c3d034451a9e39ebfc9fe5f81fd929fd3bf833ba922d7cca4` |
| `docs/specs/cli.json` | `eed447e6078d56e0b78355fbd9d378bafc50af803539a3c1ed70559f4a6c5b25` |
| `docs/specs/matrix.json` | `1ec3109b949389083cc07f17e7b4eea637a7b7c2f00fc8f243f63fa0ab8a3af5` |
| `docs/specs/REQ-DX-V1.md` | `007c84e3a29c8ff2e0f552fcd9374c05e1f51d79349d223a946d1b737672fa0f` |
| `docs/architecture/polyglot-v1.md` | `9210d444c8aad897f5ce89d6c32203e5ed1c2e3ca0825384b0330b2ee6938f6a` |

Delete this implementation plan and its task JSON when the covered work ships. Keep measured evidence, support boundaries and still-open owner choices in durable docs.

# V1 system findings

Baseline: main `8ef9aa5a4` (#168), audited on 2026-09-26. This is an open findings register, not an implemented behavior spec. Corrections implemented locally are labeled and remain unshipped until merged. [Roadmap](ROAD-TO-V1.md), [execution guide](V1-IMPLEMENTATION.md), [task definitions](V1-SYSTEM-HARDENING.json), [proposed model](../architecture/polyglot-v1.md).

Confirmed means reproduced against the main binary or established by the cited code path; the evidence column distinguishes them. The earlier isolated run passed 6 identity, 5 schema-guard, 10 release-command and 14 durable-release tests. Those tests do not establish the missing producer/consumer or ecosystem coverage. No full `just ci`, hosted publication, competitor execution or fresh-repo timing run was performed in that audit.

## Implemented locally, awaiting merge

These corrections are not shipped. [Batch verification](V1-VERIFICATION.md) records local results and remaining checks.

| Finding | Correction and evidence | Plan | Contract/area |
| --- | --- | --- | --- |
| SYS-ENV | Unchanged decide stdout reproduced E281 at the baseline. The CLI accepts the decide report or strict bare wire while preserving typed validation. Review also found strict post-move verification rejected the action's own update with E148; the model now permits only that exact applied-head transition. Real-binary, model and shell action tests cover the boundaries and unrelated-change negatives. | T01 | REL-PR-02, REL-PR-04, REL-PR-07 |
| SYS-INSTALL | Auto installation honors explicit/action-ref versions, including inside Callisto source. Local compilation requires explicit source selection and native workspace ownership checks. Isolated installer fixtures cover consumer names, pins, source rejection and attestation/fallback behavior. | T11 | REL-PR-06 |
| SYS-SCHEMA | Custom string serializers now have string-shaped, named schema definitions. A full JSON Schema validator checks every exposed schema target against real report/durable payloads and malformed nested values; full CI passes. | T02 | CLI-SCHEMA-01, CLI-OUT-05, REL-CMD-12 |
| SYS-PYDEC | Durable release entries, operations, slots and bound registry observations decode versions with the owning distribution's grammar. Genuine Python stable release plan/execute/rerun tests preserve PEP 440 values and reject foreign rosters; independent review found no must-fix. | T03 | REL-AUTH-01/03 |
| SYS-CASCADE | Dependency declarations now retain both local key and native target; graph walking and shared cascade/floor/snapshot rewriting use the declaration's manifest ecosystem. Cargo aliases, inherited root declarations, promoted dual Cargo/npm packages and normalized Python graph names have regressions; independent review cleared T04 and full CI passes. | T04 | VER-CAS-07, VER-PRE-09 |
| SYS-PYPRE | Pre-release baselines now resolve their historical/display identity before native grammar validation. Validation uses the original JSON spelling, so PEP 440 fallback cannot normalize invalid SemVer input; Python stable cycles, promoted/historical keys, malformed values, exit and resume are covered. | T03-PRE | VER-PRE-01/03/06/10 |
| SYS-INHERITED, SYS-ROOT | Historical release verification now reads effective versions from Git blobs, including Cargo workspace inheritance; normalized root/member/changelog paths are checked against the claimed roster. Tests cover authority-only root changes, unclaimed owners and unsafe paths. | T03-HISTORY | REL-AUTH-02, VER-APPLY-13 |
| SYS-PNPM | Dependency state now retains pnpm workspace suffixes, alias targets and relative paths. Explicit SemVer ranges are checked and safely rewritten; aliases preserve keys, relative references resolve only to admitted members, and native pack/frozen-install tests cover version and snapshot flows. | T04-PNPM | WS-ID-10, VER-CAS-05/06 |
| SYS-LOCK | Lockfile refresh and staging now include every actual version/dependency/platform/optional-dependency write ecosystem, including dual manifests and workspace-root Cargo writes. Cross-ecosystem and dependency-only regressions pass in full CI. | T04-LOCK | VER-APPLY-14, VER-PRE-08 |
| SYS-VERSION-RECOVERY | Consumed changesets and pre-state deletions are deferred until lock refresh and generated-state staging succeed. Partial-run detection checks staged effective versions, permits metadata-only edits and handles the examined prerelease exit and inherited-Cargo cases; regressions cover untracked inputs, failed deletion staging, staged/restored versions, exit-state stable bumps, and a fresh E121 refusal after pre-state deletion staging fails. | T05 | VER-APPLY-08/10/13/15/16 |
| SYS-BINS, SYS-NONCARGO | Cargo executable discovery covers explicit and automatic bin targets. npm/Python executables are rejected as unsupported generated artifact builders before writes, while registry-only setup remains available. Focused init and manifest tests plus full CI pass. | T10-BINS | CLI-INIT-01/02/12, CLI-WF-04 |
| SYS-AUTH | Generated release actions configure npm auth in a mode-0600 temporary user config using `${NPM_TOKEN}` interpolation (the secret is not written to disk), map PyPI credentials to Twine, support Bun's token contract, and filter release credentials including `NODE_AUTH_TOKEN` from Python build subprocesses. Native package-manager lifecycle hooks retain normal package-owner-defined behavior; Callisto does not suppress or rewrite them. Fake-tool and environment-filter tests pass; generated workflow tests keep secrets out of version-pr/plan/build. | T10-AUTH | CLI-WF-03/04; ADR 4 |

## Open confirmed defects

| Finding | Evidence and consequence | Plan | Contract/area |
| --- | --- | --- | --- |
| SYS-OWNER | `sole_owner_at`: current first-manifest path existence is not historic name/full-roster ownership. Reproduced loss of a bare tag after sibling rename or addition of a new primary canonical manifest. | T06-HISTORY | WS-ID-03; historical migration |
| SYS-DEMOTION | `tags.rs`: removing a same-named sibling loses a newer qualified tag and chooses an older bare release. Reproduced; inference can replay already-released work. | T06-HISTORY | WS-ID-03; historical migration |
| SYS-RETRY | `derive_unreleased_decision`: local tags exclude a package before remaining operations are observed. Reproduced tag creation followed by push failure, then exit 0 with `Nothing to release.` and no receipt. Generated simple CI uses this path; Callisto's own committed-roster CI does not. | T08-RETRY | REL-RERUN-02; REL-CMD-01/08 need coordinated selection changes |
| SYS-BARRIER | `release/derive.rs`: a dual Cargo/npm owner creates a shared physical tag/GitHub publication after one distribution but before the other's publish. Reproduced plan ordering; per-distribution gates do not enforce the owner's completion barrier. | T08 | REL-ORD-01 |
| SYS-NAPI | `commands/init.rs`: generated native builds and placement use workspace-wide napi selection rather than the verified intent. An unrelated addon can block a Cargo-only release. Source-confirmed. | T10 | CLI-WF-04; committed roster authority |
| SYS-MATURIN | `Package::version_grammar`: two fresh co-located Cargo/Maturin repos, with matching static versions and Cargo-owned dynamic Python versions, both reproduce E007 from init/status. Separate-directory fixtures did not cover this layout. | T07 | WS-ID-09; proposed version authority |
| SYS-PYFILES | `provider/registry.rs` returns Exact for any matching non-yanked Python file; `provider/mod.rs` adopts it as AlreadySatisfied. `registry_argv.rs` builds both wheel and sdist. Source-confirmed partial-upload retry gap; no live fault-injection run yet. | T09 | REL-PUB-04/08, REL-RERUN-01/04 |
| SYS-PYROUTES | `PreparedRegistryBinding.endpoint` is reused for simple-index observation and Twine upload. Source-confirmed single-route assumption; servers with distinct read/upload routes cannot generally use one override. Public defaults use separate native routes. | T09-ROUTES | REL-PUB-04/08; private-index policy |

## Integration risks and evidence gaps

- Status changeset attribution and well-formedness checks still scan display ids through `PackageId::resolve_unique`, while version aggregation uses `IdentityIndex::resolve_id`. Source-visible resolver divergence can reject a valid secondary native name or accept an unrelated qualified selector. Add a paired status/version fixture and migrate both status helpers with the other selector consumers (T06).
- Native npm lifecycle hooks can initiate effects outside Callisto's release-operation graph. Their behavior is package-owner-defined and is not inspected, rewritten, suppressed or rejected by Callisto; this is an explicit ownership boundary, not an automatic-enforcement defect.
- Registry version existence is not proof of identical package bytes, complete Python files or the desired npm dist-tag. Artifact-set completion is a correctness fix (T09); universal checksum enforcement and channel promotion remain owner decisions.
- PyPI yanking is per file. The current classifier treats any matching yanked file as Absent even alongside non-yanked files. A yank is not deletion or permission to overwrite; confirm conflict/partial semantics with the required-output model (T09).
- Cargo and PyPI real local registry tests exist in `release_real_registry_e2e_tests.rs`; they drive publisher argv directly, bypass production binding/observation in those paths, and can skip when tools are missing. npm has broader end-to-end coverage. Record exercised versus skipped paths; do not mark the missing Cargo test as the remaining v1 blocker (T12).
- Fresh local Cargo/npm/PyPI repositories completed documented setup through release dry-run; npm packing and Cargo package inventory passed. Python artifact build was blocked on sandbox network access to `setuptools`, and no timings, credentials, registry publication or hosted release were exercised. The under-ten-minute first-published-release goal therefore remains unverified (T12-DX).
- Go v2/tag publication and NuGet normalized versions/RID/TFM assets are architecture examples only, not production support or new v1 gates (T07/T08/T12).

## Owner policy, not a defect verdict

Commit inference can produce pending bumps while `hasChangesets` is false; managed release-PR decisions then return noop. Committed verification also requires a consumed changeset, as ADR 2 states. Decide whether inferred-only/manual version releases belong in managed CI before changing that authority. Do not silently accept a reconstructed roster.

Authenticated private PyPI/HTML-only indexes, npm channel promotion, universal registry checksum requirements and automatic wrapper bump severity also need a support/policy decision. The proposed model keeps room for them without making them required implementations.

A failure to stage `pre.json` removal after stable finalization leaves the file in place, and a fresh `version` invocation refuses with E121 and manual recovery guidance. This fail-closed outcome is the selected policy, not an automatic-retry feature; an end-to-end regression verifies it.

## v0.9 handoff

Merge the verified T01 correction to unblock release-PR refresh. Then require fresh release-source/decision agreement and passing workflow checks before merging the refreshed PR; do not merge the stale #113 decision as a workaround. [Run 36281315129](https://github.com/orin-dx/callisto/actions/runs/36281315129) records the E281 failure that prevented refresh after later merges. No claim that E281 is the only possible publication blocker is established until the actual self-release succeeds.

The publication intent, artifact-manifest and receipt files are written without the stdout report envelope, so the same wrapper mismatch was not found there. That source inspection does not replace executing the publish path.

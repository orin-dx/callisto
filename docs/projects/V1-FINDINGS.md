# V1 system findings

Baseline: main `07154021c` (0.10.2). This is an open findings register, not an implemented behavior spec: it lists unshipped defects and evidence gaps only. [Roadmap](ROAD-TO-V1.md), [execution guide](V1-IMPLEMENTATION.md), [task definitions](V1-SYSTEM-HARDENING.json), [proposed model](../architecture/polyglot-v1.md).

Confirmed means reproduced against the main binary or established by the cited code path; the evidence column distinguishes them. No hosted publication, competitor execution or fresh-repo timing run has been performed.

## Open confirmed defects

| Finding | Evidence and consequence | Plan | Contract/area |
| --- | --- | --- | --- |
| SYS-MATURIN | `Package::version_grammar`: two fresh co-located Cargo/Maturin repos, with matching static versions and Cargo-owned dynamic Python versions, both reproduce E007 from init/status. Separate-directory fixtures did not cover this layout. | T07 | WS-ID-09; proposed version authority |
| SYS-PYFILES | `provider/registry.rs` returns Exact for any matching non-yanked Python file; `provider/mod.rs` adopts it as AlreadySatisfied. `registry_argv.rs` builds both wheel and sdist. Source-confirmed partial-upload retry gap; no live fault-injection run yet. | T09 | REL-PUB-04/08, REL-RERUN-01/04 |
| SYS-PYROUTES | `PreparedRegistryBinding.endpoint` is reused for simple-index observation and Twine upload. Source-confirmed single-route assumption; servers with distinct read/upload routes cannot generally use one override. Public defaults use separate native routes. | T09-ROUTES | REL-PUB-04/08; private-index policy |

## Integration risks and evidence gaps

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

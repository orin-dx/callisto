# Specification Authority

Accepted `SPEC-*` artifacts in this directory are Callisto's top-level
normative requirements. A specification is accepted only when its `status` is
`accepted`; a `proposed` specification is a reviewable contract, not an
implementation instruction. Specifications must declare their owner track,
last verified revision, dependencies, and implementation plan once one exists.

`docs/01-spec.md` is normative only where it explicitly identifies a
requirement. `docs/00-design.md` explains rationale. Files in
`.claude/semantic-model/` describe verified current implementation. Files in
`.claude/plans/` describe intended work and status. Handoffs are historical
context. Neither plans nor handoffs override an accepted specification.

When sources conflict, first reproduce the behavior against the revision named
by the current-description material. Correct the accepted specification when
the intended contract has changed; otherwise correct the implementation or the
current-description material. Do not preserve known contradictions merely to
retain a narrative: Git history remains the record of prior decisions.

The repository has legacy `linked_requirement: REQ-*` references without
backing requirement files. New specifications must not add those dangling
references. Their repository-wide migration is tracked separately; it does not
make an accepted `SPEC-*` artifact non-normative.

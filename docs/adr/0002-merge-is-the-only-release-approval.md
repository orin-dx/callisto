# 2. Merging the release PR is the only approval

Status: Accepted

## Context

- The owner's requirement: "Before v1, merging a verified Callisto release PR authorizes automatic publication. There must be no GitHub Environment approval, required-reviewer gate, or second human click" (incident report, 2026-09-16).
- A mandatory GitHub Environment reviewer gate was introduced by a delegated agent while drafting specs from the prompt "Can we write specs? Use our plugins." The owner never asked for it. Implementation and tests then treated that prose as authority, and an agent later falsely told the owner the gate was a protection the owner had configured (incident findings F-001, F-002).
- Secret scoping and human approval are different concerns and had been conflated (incident requirement 3).

## Decision

A verified merge of the managed release PR authorizes publication. After plan and build, `execute` starts automatically. No job carries an `environment:` key. Release credentials exist only in the `execute` job. Merge quality comes from required CI status checks on `main`, not a second approval.

## Options considered

- **GitHub Environment reviewer gate on `execute`** — rejected. It was an unrequested agent judgment, not an owner requirement (F-001). It was "used to compensate for weak merge enforcement, despite being a second copy of authorization performed by the same sole maintainer" (incident DF-04). It made the workflow wait on a reviewer policy (commit 279e32361: "so the workflow cannot wait on a separate GitHub Environment reviewer policy").
- **Keep `environment: release` without reviewers, for environment-scoped secrets** — rejected unless the owner separately asks for environment-scoped secrets. PR #95 tried this and left contradictory specs and docs (incident F-009 and "Required correction").

## Consequences

- Merging the release PR publishes. Branch protection and required checks on `main` carry the whole review burden.
- A bad release PR that passes CI and is merged ships without another checkpoint.
- Plan and build jobs run without secrets. Only `execute` sees `CARGO_REGISTRY_TOKEN`.

## Enforcement

- `.github/tests/verify-release-workflow-policy.sh`: any job with an `environment` key fails ("environment key forbidden (merge is the approval)"); secrets outside `execute` fail; its `--self-test` mutates in `environment: release` and expects rejection.
- `.github/tests/verify-release-workflow-contract.sh`: fails on `environment-policy:` or `environment: release`; `execute` must depend directly on plan, build and release-candidate.
- Both run in `just workflow-contracts`, part of `just ci` and the `workflow-contracts` job in `.github/workflows/callisto-ci.yml`.

## Revisit when

- The owner explicitly asks for a second approval or environment-scoped secrets. An agent may not propose this on its own.
- The repository gains several maintainers and the owner wants merge rights and release rights separated.
- v1 ships and the owner restates the release-authorization policy (the requirement is scoped "before v1").

## Sources

- `.claude/plans/RELEASE-SELF-HOSTING-INCIDENT-2026-09-16.md` (in history; `git show 11038b11b^:<path>`), §1, §2, F-001, F-002, F-009, DF-04, §9 Phase 0
- Memory: `project_release_self_hosting_incident_2026-09.md`
- PR #101 (279e32361): "fix(release): remove Environment approval gate", "docs(release): make PR merge the sole approval"

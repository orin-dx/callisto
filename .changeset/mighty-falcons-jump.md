---
callisto-cli: minor
---

**Release PRs open for releases inferred from commits.** A package with `release-trigger = "auto"` now gets a release PR from its commits alone; it no longer needs a changeset to be released.

- `release-pr decide` opens a PR whenever `callisto version` would version a package, after groups and dependents.
- A changeset that cannot be parsed now fails `release-pr decide` with the error `version` reports, instead of a green run that releases nothing.
- `status` no longer warns `inferred-release-without-changeset`; that diagnostic code is gone.

**Scripting:** `version --emit-decision` writes decision schema 3, which adds `consumedChangesets`. Release verification (E165) checks that the release commit deletes exactly those changesets. Schema 1 and 2 decisions still read.

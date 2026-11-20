---
callisto-cli: minor
---

**`callisto init` migrates a Changesets config.**
Move an `@changesets/cli` 2.x or 3.x repository to Callisto without rewriting its `fixed` groups or the workflow branch by hand.

- Why it matters: `init` reads `.changeset/config.json` and tells you everything it could not carry over, instead of leaving those settings silently inert.
- Adopt: run `callisto init` in the repository.
  - `fixed` becomes `[[fixed-group]]` entries, with globs expanded to the npm packages that exist now.
  - `baseBranch` becomes the branch filter of the generated workflow.
  - `linked`, `ignore`, `updateInternalDependencies` and every other key get one warning each.
- Limits: add packages created after `init` to the groups by hand. `linked` is not translated, because a Callisto linked group bumps every member on each release.

**Action:** if `access` is `restricted` or absent and a scoped npm package has no `publishConfig.access`, `init` refuses (E342) unless you pass `--yes`, because Callisto publishes such packages as public. Set `publishConfig.access` in each listed package.json, or pass `--yes`.

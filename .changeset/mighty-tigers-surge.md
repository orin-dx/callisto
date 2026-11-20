---
callisto-cli: minor
---

**`callisto init` migrates a Changesets config.**
Switch an `@changesets/cli` 2.x or 3.x repository to Callisto without rewriting its groups or release branch by hand.

- **Why it matters:** `init` reads `.changeset/config.json` and reports every setting it did not carry over, so none goes silently inert.
- **How to adopt:** run `callisto init` in the repository.
  - Translated:
    - `fixed` becomes `[[fixed-group]]` entries, with globs expanded to the npm packages that exist now.
    - `baseBranch` becomes the branch filter of the generated release workflow.
  - Reported, not translated:
    - `linked`, `ignore` and `updateInternalDependencies`.
    - Any other non-empty key, such as `changelog` or `commit`.
- **Limits:**
  - Add packages created after `init` to the fixed groups by hand.
  - `linked` is not translated: a Callisto linked group bumps every member on each release, while Changesets bumps only members with changesets.

**Action:** if `access` is `restricted` or absent and a scoped npm package has no `publishConfig.access`, `init` refuses (E342) unless you pass `--yes`, because Callisto publishes such packages as public. Set `publishConfig.access` in each listed package.json, or pass `--yes`.

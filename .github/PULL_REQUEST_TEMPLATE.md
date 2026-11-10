## Summary
- What changed (1-3 bullets)
- Why it was needed

## Type of change
- [ ] Bug fix
- [ ] New feature
- [ ] Breaking change (changes CLI flags, output format, or `callisto.toml` schema)
- [ ] Documentation only

## Changeset
- [ ] Added or updated the changeset for this feature (what a user sees, why it matters, what to do), or this change needs none: a fix to something unreleased, an internal change, tests or docs only. Say which below.

## Docs
- [ ] A change to a flow, flag, config key or spec'd behavior updates the spec (`docs/specs/`), `ARCHITECTURE.md` or the matching `docs/architecture/` page, and `docs/config.md` / `docs/errors.md` where they apply, or says here why none applies

## Test Plan
- [ ] `just ci` passes locally (format, clippy, tests, WASM check, `cargo deny` audit)
- [ ] Edge cases covered (e.g. malformed manifest, cyclic workspace dependency, WASM target)
- [ ] Regression check against existing fixtures in `callisto-fixtures`

## Related
Closes #<issue>

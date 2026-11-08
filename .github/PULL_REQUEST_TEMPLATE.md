## Summary
- What changed (1-3 bullets)
- Why it was needed

## Type of change
- [ ] Bug fix
- [ ] New feature
- [ ] Breaking change (changes CLI flags, output format, or `callisto.toml` schema)
- [ ] Documentation only

## Changeset
- [ ] Added a changeset (`callisto add`), or explain below why this change doesn't need one (docs-only, internal refactor with no observable behavior change)

## Docs
- [ ] A change to a flow, flag, config key or spec'd behavior updates the spec (`docs/specs/`), `ARCHITECTURE.md` or the matching `docs/architecture/` page, and `docs/config.md` / `docs/errors.md` where they apply, or says here why none applies

## Test Plan
- [ ] `just ci` passes locally (format, clippy, tests, WASM check, `cargo deny` audit)
- [ ] Edge cases covered (e.g. malformed manifest, cyclic workspace dependency, WASM target)
- [ ] Regression check against existing fixtures in `callisto-fixtures`

## Related
Closes #<issue>

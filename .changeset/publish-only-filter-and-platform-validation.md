---
callisto-graph: patch
---

**Fix: `--package` ecosystem collisions and unvalidated platform dependencies**

- `--package` now resolves names by ecosystem-aware identity instead of bare string. A Cargo crate and an npm package sharing a name no longer both match one `--package` request; a genuinely ambiguous bare name now errors instead of silently including both (qualify it with `npm:name` to disambiguate).
- `depends_on_platforms` is now validated against the final plan. A main npm package whose platform dependency is missing — misconfigured, or excluded by `--package` — now fails the plan instead of publishing a broken `optionalDependencies` reference.
- `--package` naming a real package with nothing pending now returns a precise reason (not a release candidate, or no dispatchable publish target) instead of the generic "unknown package" message.

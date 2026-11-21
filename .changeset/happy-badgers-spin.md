---
callisto-cli: patch
---

**Dual-published packages no longer hit a false cycle error.** A release of packages published to both Cargo and npm now plans when their dependencies point opposite ways in the two ecosystems.

- Why it matters: crate `a` depending on crate `b`, with npm `b` depending on npm `a`, failed with `E104` although each registry orders cleanly.
- Limits: a runtime, build or optional cycle among the published crates and packages, including through a dependency published only to the other registry, still fails with `E104`.

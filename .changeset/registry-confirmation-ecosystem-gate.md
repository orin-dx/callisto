---
callisto-graph: patch
---

**Extract the registry-confirmation ecosystem gate into a single, testable source of truth**

`dispatch_registry` decided which ecosystems get a post-publish registry-confirmation check inline, in the same match arm that ran the check itself -- so nothing directly proved Cargo was actually wired in (only the classification logic it calls was unit-tested). That's exactly the shape of gap that let Cargo's confirmation go missing in the first place.

The decision is now its own pure function, `ecosystem_requires_registry_confirmation`, which `dispatch_registry` calls directly rather than re-deciding inline. A test on that function is now a direct proof of the runtime gate, not a parallel copy of it -- verified by temporarily reintroducing the original gap and confirming the test catches it.

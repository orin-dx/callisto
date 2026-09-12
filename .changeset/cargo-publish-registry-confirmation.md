---
callisto-graph: patch
---

**Confirm a Cargo publish landed on the registry before marking it done**

`dispatch_registry` only confirmed npm publishes against the registry after a successful publish call; Cargo went straight from "cargo publish exited 0" to `Published`, with no check that crates.io's index had actually caught up. If a dependent crate's publish ran before that propagation finished, its own `cargo publish --locked` verification build could fail to resolve the dependency, and the failure wouldn't be recognized as recoverable propagation lag.

Cargo publishes are now confirmed the same way npm's are: after a successful `cargo publish`, `cargo info <pkg>@<version>` must also confirm the registry shows it before the operation is marked `Published`. If it doesn't yet, the operation stays `Attempting` and reports `RegistryPublishUnconfirmed` -- re-running reconciliation once the registry catches up resolves it, with no need to regenerate the release intent.

PyPI's publish path has the same gap and is deliberately left open: this workspace has no PyPI package or credentials to verify a fix against.

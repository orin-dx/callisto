---
callisto-cli: patch
---

**Dependents are no longer released for a version their dependency skips**: a dependent is released only when its range rejects the version its dependency is finally released at.

- Before, a fixed or linked group could move a member past a version the dependent's range rejected, and the dependent was still released with an unneeded patch. For example, a `>=2.0.0` consumer of a group member released at 2.0.0.

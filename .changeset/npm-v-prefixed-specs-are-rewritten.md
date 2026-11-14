---
callisto-cli: patch
---

An npm dependency written with a `v` prefix or `~>` (`"v1.2.3"`, `"=v1.2.3"`, `"^v1.2.3"`, `"~>1.2.3"`) is now tested against a bumped dependency, so its dependent is released and its range updated.

- Before, these specs were treated as opaque: when the dependency moved to 2.0.0, the dependent was not bumped, not rewritten and got no warning, and kept pointing at the old version.
- Now they behave like `1.2.3`, `^1.2.3` and `~1.2.3`. A rewritten spec is written without the `v` (`^v1.2.3` becomes `^2.0.0`) and `~>` is written as `~`.
- You do not need to do anything.

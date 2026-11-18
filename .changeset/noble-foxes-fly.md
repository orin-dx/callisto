---
callisto-cli: patch
---

**Python requirements on an epoch version keep the epoch.** Releasing a Python package that sits in a PEP 440 epoch no longer rewrites a dependent's `>=2!0.4.8` to `>=0.4.9`, which silently accepted every epoch-0 version such as 99.0.0.

- Why it matters: the rewritten range now admits the new release and still rejects the versions the original rejected.
- What changes: `==`, `~=`, `>=` and `>=X,<Y` requirements write `N!` on every bound, including the upper bound.
- Limit: a Poetry caret or tilde has no epoch form, so one that needs rewriting for an epoch release fails the run with E319.

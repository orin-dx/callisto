---
callisto-format: patch
---

`SemVerVersioning::bump`/`Pep440Versioning::bump` and `SemVerVersioning::bump_prerelease`/`Pep440Versioning::bump_prerelease` each independently implemented the same major.minor.patch zero-guard bump arithmetic and the same trailing-counter increment-and-reassemble logic, differing only in representation (`Version` vs. a raw `(u64, u64, u64)` triple). This duplication already cost two independent bug-fix commits for the same arithmetic (one on the SemVer copy, one on the PEP 440 copy).

Extracted `bump_release_triple` (the zero-guard major/minor/patch arithmetic) and `next_counter` (the prerelease-counter-increment logic) as shared helpers used by both grammars. Pure refactor: no behavior change, all existing tests pass unchanged.

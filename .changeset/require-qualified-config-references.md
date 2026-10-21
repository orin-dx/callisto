---
callisto-cli: minor
---

`callisto.toml` now requires the ecosystem on every package it names: `cargo/foo`, `npm/foo` or `pypi/foo`. A bare `foo` in a group, a `[[package]]` or `[[package-set]]` `match`, or a product or artifact package is rejected with E317 showing the form to write. A package-set glob names one ecosystem, so a rule for several ecosystems is one rule each. Changeset entries keep native names.

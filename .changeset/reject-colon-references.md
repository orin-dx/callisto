---
callisto-cli: minor
---

Package references now take the form `ecosystem/name`, such as `cargo/foo`, `npm/foo` or `pypi/foo`. The colon form, such as `cargo:foo`, is rejected with E316 naming the slash form, in `--package`, changeset entries, `pre.json` keys, group members, package rules, and the product and artifact packages. A bare name is rejected with E317 showing the form to write, in `callisto.toml` (a group, a `[[package]]` or `[[package-set]]` `match`, a product or artifact package) and in `callisto add --package` and `callisto matrix --package`, so a name can no longer become ambiguous when a package in another ecosystem later takes it; `release --package` already required it. A package-set glob names one ecosystem, so a rule for several ecosystems is one rule each. Changeset entries and `pre.json` keys keep native names.

---
callisto-cli: minor
---

A package written with a colon, such as `cargo:foo`, is now rejected with E316 naming the slash form, `cargo/foo`. This applies to `--package`, changeset entries, `pre.json` keys, group members, package rules, and the product and artifact packages. Bare names and the slash form work as before.

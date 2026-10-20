---
callisto-cli: patch
---

A `[[fixed-group]]` or `[[linked-group]]` that lists one package under two of its names, such as a Cargo crate and its npm name in a dual package, is now rejected with E250 instead of being accepted silently.

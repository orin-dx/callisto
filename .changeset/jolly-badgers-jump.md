---
callisto-cli: patch
---

A `[[package]]` or `[[package-set]]` rule is now matched against a package's name in the rule's own ecosystem. For a dual Cargo and npm package whose npm name differs, such as the napi package `@orin-dx/michi` in a directory called `michi-node`, `npm/michi-node` used to apply to it silently while a `[[fixed-group]]` rejected the same spelling. It now matches nothing, the matched-no-packages warning suggests `npm/@orin-dx/michi`, and a group member's E108 does too.

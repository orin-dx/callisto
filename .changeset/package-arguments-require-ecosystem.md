---
callisto-cli: minor
---

`callisto add --package` and `callisto matrix --package` now require the ecosystem, such as `cargo/foo:minor` and `npm/foo`. A bare name is rejected with E317 showing the form to write, so a name can no longer become ambiguous when a package in another ecosystem later takes it. `release --package` already required it.

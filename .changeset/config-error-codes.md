---
callisto-cli: minor
---

Each configuration mistake now has its own error code, so a script or a person can tell them apart.

- E254 is retired. A bad `publish-to` is E330, `release-trigger` E331, `cascade.mode` E332, `changelog.layout` E333, a registry `kind` E334, `publish-url` on a non-PyPI registry E335, `url` on a Cargo registry E336, a registry timeout E337, and an unparsable package `match` E338. Before, all of these reported E254.
- A registry `kind` that does not match the package's ecosystem is E339. Before, it reported E171, which means an internal defect, so it looked like a bug in Callisto.
- Exit codes are unchanged.

Breaking: scripts that match E254 for these values, or E171 for a registry kind mismatch, must match the new codes.

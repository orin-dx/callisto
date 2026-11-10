---
callisto-cli: patch
---

The release PR says which packages are versioned but not published.

- Under "Merging will publish", a line such as ``Versioned, not published: `callisto-fixtures` (no publish target).`` names each package that is bumped but has nowhere to publish, such as a private package that moves with its fixed group.
- Before, that package sat in the package table next to the ones that publish, with nothing saying it goes nowhere.
- For scripts: the `--emit-preview` file lists these packages as `versionedOnly` under `publish`.

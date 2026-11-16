---
callisto-cli: patch
---

A `pnpm.overrides` pin on a workspace package now follows the package's new version, as `overrides` and `resolutions` already did.

- Before, `pnpm.overrides` kept the old version after a release, so pnpm kept installing the old package even though the dependency range had moved.
- Now a plain range in `pnpm.overrides` is rewritten with the dependency, in a manifest that declares that dependency. Values that point elsewhere (`$name`, `npm:`, `catalog:`) are left alone.
- Pins in a root `package.json` that does not declare the dependency are still not changed. If you pin a workspace package there, update it by hand.

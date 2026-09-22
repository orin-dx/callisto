---
callisto-cli: minor
---

**npm platform packages version with their owner**

An npm package with `os`/`cpu` that another npm package lists in `optionalDependencies` is now a platform manifest of that package, not a package of its own: it gets no tag, takes its owner's version, and the owner's `optionalDependencies` pins follow. This covers napi addons and native CLIs shipped esbuild-style, including `npm/<platform>` directories outside the workspace globs, which were previously ignored. Platform packages no longer need listing in a `[[fixed-group]]`. A workspace platform package with no single owner is reported as `platform-package-without-owner`.

A directory with both `Cargo.toml` and `package.json` now releases its npm side under the `package.json` name instead of the crate name.

`callisto release` publishes each attached platform package as a `platformPublish` operation of its owner, by directory (`npm publish <dir>` from the workspace root, whatever the package manager, since `--filter`/`workspace` only reach workspace members), with the owner's registry and access. Every platform publish must succeed before the owner's own npm publish runs; platform packages get no tag or GitHub Release. Release intents move to schema version 4: an intent planned by an earlier Callisto must be re-planned.

`plan-publish` lists attached platform packages by directory, and no longer reports a package that depends on a platform owner as missing a platform dependency.

Breaking (pre-1.0):
- A `Cargo.toml` + `package.json` directory whose names differ changes npm release identity, for example `npm/michi-node` becomes `npm/@orin-axi/michi`. Update `--package` selections, and re-run `callisto version --emit-decision` for any release-decision file committed before upgrading.
- A changeset naming an attached platform package (for example `"@s/cli-linux-x64-gnu": patch`) now reports an unknown package. Name the owner instead.

---
callisto-cli: minor
---

**npm platform packages version with their owner**

An npm package with `os`/`cpu` that another npm package lists in `optionalDependencies` is now a platform manifest of that package, not a package of its own: it gets no tag, takes its owner's version, and the owner's `optionalDependencies` pins follow. This covers napi addons and native CLIs shipped esbuild-style, including `npm/<platform>` directories outside the workspace globs, which were previously ignored. Platform packages no longer need listing in a `[[fixed-group]]`. A workspace platform package with no single owner is reported as `platform-package-without-owner`.

A directory with both `Cargo.toml` and `package.json` now releases its npm side under the `package.json` name instead of the crate name.

`plan-publish` lists attached platform packages by directory, and no longer reports a package that depends on a platform owner as missing a platform dependency.

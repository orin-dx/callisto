---
callisto-cli: minor
---

**Generated release workflows build split napi layouts**

`callisto matrix` adds `manifestPath` to a napi target whose addon crate is outside the npm package's directory (e.g. `packages/napi` with the crate in `crates/binding`): the workspace `cdylib` depending on `napi` whose lib name equals `napi.binaryName`. The generated workflow passes it to `napi build --manifest-path`. No single match fails with E204, listing the candidates.

The generated workflow also runs `npx --package @napi-rs/cli@3.10.4 napi ...`: the bare `npx @napi-rs/cli@3.10.4` form fails because the package has two bins.

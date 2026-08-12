# callisto matrix — auto-discovery specification

**Status:** PLANNED (Phase 3 / v0.5)
**Date:** 2026-08-09
**Repo:** `github.com/orin-dx/callisto`
**Companions:** `docs/01-spec.md` (crate spec), `docs/00-design.md` (canonical design)

---

## 1. Purpose and scope

The `callisto matrix` subcommand reads build targets and runtime version constraints directly
from workspace manifests and emits a structured JSON report that drives CI matrix strategies
without any hand-maintained YAML.

Manifest fields read:

| Field | Ecosystem | Notes |
|---|---|---|
| `napi.targets` | napi-rs (Node.js native) | Array of Rust target triples |
| `[tool.maturin].targets` | maturin (Python native) | Array of Rust target triples |
| `engines.node` | npm | Range string only; no discrete version expansion |
| `requires-python` | PyPI | PEP 440 range string |
| `<TargetFramework>` / `<TargetFrameworks>` | .NET | MSBuild TFM strings |
| `<RuntimeIdentifiers>` | .NET AOT | RID strings for native AOT publish |

Java (Maven/Gradle) and C# runtime versions are in scope for the `RuntimeVersionEntry` type
model (see §2) but their manifest readers are Phase 4 (Java) and Phase 5 (C# runtime
versions) work. This document marks them as **planned** at each relevant point.

### 1.1 Non-goals

See §8 for the complete enumerated list. Summary: no YAML generation, no manylinux compat-tag
inference, no external HTTP calls.

### 1.2 Relationship to §G.8.4

The existing `napi.rs` module (§G.8.4 of `docs/01-spec.md`) provides `triple_to_role` and
`role_to_triple` — the canonical, fixtured table mapping Rust target triples to
`ManifestRole::Platform` values. The matrix subcommand extends that table's scope from
drift-checking to report emission. The table itself is not duplicated; `callisto-graph`'s
`napi.rs` is the single source of truth.

---

## 2. JSON output contract — `MatrixReport`

The report is emitted as a single JSON object. All package names are the registered package
name (the `name` field in `package.json`, `[package] name` in `Cargo.toml`, or
`<PackageId>` in a `.csproj`).

```jsonc
{
  "platformTargets": {
    "<package_name>": { /* PlatformTargetGroup */ }
  },
  "runtimeVersions": {
    "<package_name>": { /* RuntimeVersionEntry */ }
  }
}
```

A workspace with no napi, maturin, or runtime-version constraints produces:

```json
{ "platformTargets": {}, "runtimeVersions": {} }
```

This is a valid, non-error output (exit 0).

### 2.1 `PlatformTargetGroup`

```jsonc
{
  "kind":    "napi" | "maturin" | "dotnet-aot",
  "source":  "<manifest field name>",  // "napi.targets", "[tool.maturin].targets", "<RuntimeIdentifiers>"
  "targets": [ /* Vec<PlatformTarget> */ ]
}
```

`source` is the raw field name as it appears in the manifest, for auditability. It is not a
path — the manifest path is implicit in the package.

### 2.2 `PlatformTarget`

```jsonc
{
  // Exactly one of these two identity fields is present, never both:
  "triple":  "<rust-target-triple>",   // napi and maturin only
  "rid":     "<dotnet-rid>",           // dotnet-aot only

  "platform":      "darwin" | "linux" | "win32" | "freebsd" | "android" | "wasi" | "unknown",
  "arch":          "arm64" | "x64" | "ia32" | "arm" | "riscv64" | "ppc64" | "s390x" | "wasm32",
  "abi":           "gnu" | "musl" | "gnueabihf" | "musleabihf" | "msvc" | null,
  // abi is null for: darwin, freebsd, android, wasi, and win32 (msvc is implicit on win32
  // and is omitted from the JSON value rather than emitted as "msvc").

  "hostRunner":    "macos-latest" | "macos-13" | "ubuntu-latest" | "windows-latest",
  "useCross":      true | false,
  "artifactName":  "native-<triple>" | "native-<rid>",
  "packageDir":    "<workspace-root-relative path>",
  "packageName":   "<registered package name>"
}
```

`packageDir` is the directory that must contain the native artifact before `callisto publish`
runs. For napi packages it is the path from the platform manifest entry; for maturin and
dotnet-aot it is the package root.

`hostRunner` is the cheapest GitHub Actions runner that can build the target. Targets that
require cross-compilation always set `useCross: true`; they still run on `ubuntu-latest`
unless the toolchain requires otherwise. See §3 for the complete per-triple table.

### 2.3 `RuntimeVersionEntry`

```jsonc
{
  "ecosystem":        "npm" | "python" | "java" | "dotnet",
  "field":            "<manifest field name>",
  "range":            "<raw field value>",
  "targetFrameworks": ["<tfm>", ...]  // dotnet multi-targeting only; null or absent otherwise
}
```

`range` is always the raw string from the manifest. No normalization, no expansion to discrete
versions. Callers that need discrete version lists must expand the range themselves using
ecosystem-appropriate tooling.

`targetFrameworks` is present and non-null only when the .NET project uses
`<TargetFrameworks>` (plural) rather than `<TargetFramework>`. It contains the split list of
TFM strings. A `<TargetFramework>` (singular) project produces `"range": "net9.0"` with
`"targetFrameworks": null`.

Java (`ecosystem: "java"`) and dotnet runtime versions (`ecosystem: "dotnet"`) are **planned**
for Phase 4/5 respectively. The type is defined now to keep the contract stable; the readers
are not yet implemented.

---

## 3. The napi-rs target triple table

All 18 targets supported by napi-rs as of 2026. This table is the authoritative mapping used
by both the drift cross-check (§G.8.4) and the matrix report emitter. Maintenance point:
`crates/callisto-graph/src/napi.rs`, function `triple_to_role`.

### 3.1 napi-rs / maturin Rust triple table

| Rust triple | platform | arch | abi | hostRunner | useCross |
|---|---|---|---|---|---|
| `aarch64-apple-darwin` | darwin | arm64 | — | macos-latest | false |
| `x86_64-apple-darwin` | darwin | x64 | — | macos-13 | false |
| `x86_64-pc-windows-msvc` | win32 | x64 | — | windows-latest | false |
| `i686-pc-windows-msvc` | win32 | ia32 | — | windows-latest | false |
| `aarch64-pc-windows-msvc` | win32 | arm64 | — | windows-latest | false |
| `x86_64-unknown-linux-gnu` | linux | x64 | gnu | ubuntu-latest | false |
| `x86_64-unknown-linux-musl` | linux | x64 | musl | ubuntu-latest | true |
| `aarch64-unknown-linux-gnu` | linux | arm64 | gnu | ubuntu-latest | true |
| `aarch64-unknown-linux-musl` | linux | arm64 | musl | ubuntu-latest | true |
| `armv7-unknown-linux-gnueabihf` | linux | arm | gnueabihf | ubuntu-latest | true |
| `armv7-unknown-linux-musleabihf` | linux | arm | musleabihf | ubuntu-latest | true |
| `riscv64gc-unknown-linux-gnu` | linux | riscv64 | gnu | ubuntu-latest | true |
| `powerpc64le-unknown-linux-gnu` | linux | ppc64 | gnu | ubuntu-latest | true |
| `s390x-unknown-linux-gnu` | linux | s390x | gnu | ubuntu-latest | true |
| `aarch64-linux-android` | android | arm64 | — | ubuntu-latest | true |
| `armv7-linux-androideabi` | android | arm | — | ubuntu-latest | true |
| `wasm32-wasi` | wasi | wasm32 | — | ubuntu-latest | false |
| `x86_64-unknown-freebsd` | freebsd | x64 | — | ubuntu-latest | true |

Notes:
- `osx-x64` (macOS Intel) maps to `macos-13`, which is the last GitHub-hosted runner with an
  Intel CPU. `macos-latest` is M1+ (arm64) only as of 2024.
- `win32` targets never carry `abi` in the JSON output; msvc is the only Windows ABI napi-rs
  supports and emitting it would be redundant.
- Darwin and freebsd targets carry no ABI field; `android` and `wasi` targets carry no ABI
  field.
- An unrecognised triple produces a warning diagnostic named `UnknownNapiTriple` and is
  excluded from `targets[]`. It never causes a hard error.

### 3.2 .NET RID table

| .NET RID | platform | arch | abi | hostRunner | useCross |
|---|---|---|---|---|---|
| `win-x64` | win32 | x64 | — | windows-latest | false |
| `win-arm64` | win32 | arm64 | — | windows-latest | false |
| `linux-x64` | linux | x64 | gnu | ubuntu-latest | false |
| `linux-arm64` | linux | arm64 | gnu | ubuntu-latest | true |
| `linux-musl-x64` | linux | x64 | musl | ubuntu-latest | true |
| `linux-musl-arm64` | linux | arm64 | musl | ubuntu-latest | true |
| `osx-x64` | darwin | x64 | — | macos-13 | false |
| `osx-arm64` | darwin | arm64 | — | macos-latest | false |

These are the eight RIDs supported for native AOT cross-platform publishing. Additional RIDs
declared in `<RuntimeIdentifiers>` that are not in this table produce an
`UnknownDotnetRid` warning diagnostic and are excluded from `targets[]`.

---

## 4. Artifact naming convention

```
napi/maturin:  artifactName = "native-{triple}"
               example: "native-aarch64-apple-darwin"

dotnet-aot:    artifactName = "native-{rid}"
               example: "native-linux-arm64"
```

This naming is the contract between the build phase and the publish phase:

- **Build phase**: `callisto-build-action` (§7) uploads an artifact under this name after a
  successful compilation. The artifact contains exactly one binary file at the artifact root:
  a `.node` file (napi), a wheel `.so`/`.dylib`/`.dll` (maturin), or a self-contained
  executable (dotnet-aot).
- **Publish phase**: the orchestrating action (§6) downloads each artifact by `artifactName`
  and places the binary into `packageDir` before `callisto publish` runs. `callisto publish`
  then finds the binary in the expected location and includes it in the registry upload.

The naming scheme is chosen to be unique within a single workflow run (no two entries in
`platformTargets` share a `triple` or `rid`) and stable across runs (so artifact caching
strategies based on the name are safe).

---

## 5. `callisto matrix` CLI surface

```
callisto matrix [--package <name>] [--format json|table]
```

### 5.1 Flags

| Flag | Default | Description |
|---|---|---|
| `--package <name>` | all packages | Restrict output to a single package by registered name |
| `--format json\|table` | `json` | Output format. `table` renders a human-readable ASCII table |

### 5.2 Exit codes

| Code | Condition |
|---|---|
| 0 | Success, including the case where the report is empty (no napi/maturin/runtime targets found) |
| 1 | A manifest read error occurred (file unreadable, malformed TOML/JSON/XML) |

An empty report is not an error. A workspace that has no napi packages, no maturin packages,
and no `engines.node` / `requires-python` constraints exits 0 with `{"platformTargets":{},"runtimeVersions":{}}`.

### 5.3 Output ordering

Keys in both top-level maps are sorted lexicographically by package name. `targets[]` entries
within a `PlatformTargetGroup` are sorted by triple/RID string, so the output is
deterministic and diff-friendly.

### 5.4 `--format table`

The table format is for human inspection only. Its schema is not part of the stable contract.
CI pipelines must use `--format json`.

---

## 6. GitHub Actions integration contract

The matrix feature has two touchpoints in the GitHub Actions workflow.

### 6.1 `nativeMatrix` output — the strategy matrix source

The orchestrating step (the `callisto-orchestrate` step in `callisto-action`) runs
`callisto matrix --format json` early in the publish branch (after confirming no pending
changesets) and emits the `targets[]` array as an action output named `nativeMatrix`.

```yaml
# In the orchestrate step, publish branch:
MATRIX_JSON=$(callisto matrix --format json | jq '[.platformTargets[].targets[]] | unique_by(.artifactName)')
echo "nativeMatrix=$MATRIX_JSON" >> "$GITHUB_OUTPUT"
```

Consumer workflows pipe this into a downstream build job:

```yaml
jobs:
  matrix:
    outputs:
      nativeMatrix: ${{ steps.callisto-orchestrate.outputs.nativeMatrix }}

  build-native:
    needs: [matrix]
    if: ${{ needs.matrix.outputs.nativeMatrix != '[]' }}
    strategy:
      matrix:
        target: ${{ fromJson(needs.matrix.outputs.nativeMatrix) }}
    uses: ./.github/actions/callisto-build-action
    with:
      target: ${{ toJson(matrix.target) }}
```

### 6.2 Artifact routing — placement before publish

Before `callisto publish` runs, the orchestrating step downloads each artifact by
`artifactName` and places it into `packageDir`.

Contract for artifact content: the downloaded artifact must contain exactly one binary at the
artifact root. The binary is:

- A `.node` file for napi (the output of `napi build`).
- A Python wheel `.whl` file for maturin.
- A self-contained executable (no extension on Linux/macOS, `.exe` on Windows) for dotnet-aot.

The placement step is:

```yaml
- name: Download native artifacts
  run: |
    MATRIX_JSON='${{ needs.matrix.outputs.nativeMatrix }}'
    echo "$MATRIX_JSON" | jq -c '.[]' | while read -r target; do
      ARTIFACT=$(echo "$target" | jq -r '.artifactName')
      DIR=$(echo "$target" | jq -r '.packageDir')
      gh run download --name "$ARTIFACT" --dir "$DIR"
    done
```

---

## 7. `callisto-build-action` composite action design

Location: `.github/actions/callisto-build-action/action.yml`

### 7.1 Inputs

| Input | Required | Description |
|---|---|---|
| `target` | yes | JSON string of one `PlatformTarget` object (§2.2) |

### 7.2 Dispatch on `kind`

The action reads `target.kind` to select the build toolchain:

#### kind = `napi`

```yaml
- uses: dtolnay/rust-toolchain@stable
  with:
    targets: ${{ fromJson(inputs.target).triple }}

- name: Install cross (if needed)
  if: ${{ fromJson(inputs.target).useCross }}
  run: cargo install cross --git https://github.com/cross-rs/cross

- name: Install napi-rs CLI
  run: npm install -g @napi-rs/cli

- name: Build native module
  run: |
    TARGET='${{ fromJson(inputs.target).triple }}'
    CROSS='${{ fromJson(inputs.target).useCross }}'
    PKG_DIR='${{ fromJson(inputs.target).packageDir }}'
    cd "$PKG_DIR"
    if [[ "$CROSS" == "true" ]]; then
      napi build --platform --release --target "$TARGET" --cross-compile
    else
      napi build --platform --release --target "$TARGET"
    fi

- uses: actions/upload-artifact@v4
  with:
    name: ${{ fromJson(inputs.target).artifactName }}
    path: ${{ fromJson(inputs.target).packageDir }}/*.node
```

#### kind = `maturin`

```yaml
- uses: dtolnay/rust-toolchain@stable
  with:
    targets: ${{ fromJson(inputs.target).triple }}

- uses: PyO3/maturin-action@v1
  with:
    target: ${{ fromJson(inputs.target).triple }}
    args: --release --out dist
    manylinux: auto

- uses: actions/upload-artifact@v4
  with:
    name: ${{ fromJson(inputs.target).artifactName }}
    path: dist/
```

#### kind = `dotnet-aot`

```yaml
- uses: actions/setup-dotnet@v4

- name: Publish native AOT
  run: |
    RID='${{ fromJson(inputs.target).rid }}'
    PKG_DIR='${{ fromJson(inputs.target).packageDir }}'
    cd "$PKG_DIR"
    dotnet publish -r "$RID" -c Release -o out/

- uses: actions/upload-artifact@v4
  with:
    name: ${{ fromJson(inputs.target).artifactName }}
    path: ${{ fromJson(inputs.target).packageDir }}/out/
```

### 7.3 Runner selection

The action is dispatched from a matrix whose `runs-on` is set to `target.hostRunner`. The
action itself does not override the runner; that is the matrix job's responsibility.

---

## 8. Non-goals

The following are explicitly out of scope for the matrix feature, now and in future phases
unless a separate spec extends this one:

1. **YAML generation for GitHub Actions workflows.** `callisto matrix` emits JSON data; the
   calling workflow is responsible for its own structure.

2. **Inferring maturin targets from manylinux compat tags.** The mapping from
   `manylinux_2_17_x86_64` to a set of Rust triples requires external configuration that
   maturin itself does not embed in `pyproject.toml`. The matrix reader requires explicit
   `[tool.maturin] targets = [...]`.

3. **Fetching current LTS version lists from external services.** `engines.node` and
   `requires-python` are emitted as raw range strings. Converting `>=20` to `[20, 22, 24]`
   requires an HTTP call to the Node.js release schedule or PyPI classifiers; that is outside
   callisto's hermetic-by-design scope (§13 invariant, no `reqwest`/`octocrab` dependency).

4. **Java JNI platform matrix.** The JVM is inherently portable; JNI native libraries are
   rare enough in monorepo contexts that the cross-product of JVM versions times native
   targets is not worth a generalized model. Java appears in `RuntimeVersionEntry` only (Maven
   `<java.version>`, Gradle `sourceCompatibility`), and that reader is Phase 4 work.

5. **Matrix subcommand for Go.** Go's cross-compilation story (`GOOS`/`GOARCH` env vars) does
   not follow a manifest-declared target list pattern, and Go does not produce language-native
   binary modules in the napi/maturin/dotnet-aot sense.

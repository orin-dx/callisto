# Configuration reference

Every key `callisto.toml` accepts, from `crates/callisto-graph/src/config/{raw,resolve,groups}.rs`. Unknown keys are rejected (`#[serde(deny_unknown_fields)]`).

## Top-level tables

| Table | Type | Required | Meaning |
| --- | --- | --- | --- |
| `[changesets]` | table | no | Where changesets live |
| `[cascade]` | table | no | Dependency-bump cascade policy |
| `[validation]` | table | no | Status/version validation policy |
| `[release]` | table | no | Product binary release declaration |
| `[registries]` | table of tables | no | Registry endpoint overrides |
| `[[package]]` | array of tables | no | Per-package exact-match overrides |
| `[[package-set]]` | array of tables | no | Per-package glob-match overrides |
| `[[fixed-group]]` | array of tables | no | Packages that version in lock-step |
| `[[linked-group]]` | array of tables | no | Packages that share severity, not version |
| `[init]` | table | no | Deprecated; parsed for compatibility, never read |

## `[changesets]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `dir` | string | `.changeset` | Workspace-root-relative changeset directory. Forward-slash-separated only; rejected if absolute or containing `..` (E116). |

Example: `dir = ".changeset"`.

## `[cascade]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `mode` | string (`"out-of-range"` \| `"always"`) | `"out-of-range"` | When a dependency bump forces a cascading bump on dependents |
| `bump-severity` | string (`"patch"` \| `"minor"`) | `"patch"` | Severity applied to a cascaded bump |
| `peer-escalation` | bool | `true` | Escalate peer-dependency bumps into the cascade |
| `preserve-npm-ranges` | bool | `true` | Keep existing npm semver range operators (`^`, `~`) when rewriting dependency versions |

Example (this repo's own `callisto.toml`):
```toml
[cascade]
mode = "out-of-range"
bump-severity = "patch"
peer-escalation = true
preserve-npm-ranges = true
```

## `[validation]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `allow-empty-changesets` | bool | `false` | Allow `callisto version` to run with zero pending changesets (also settable per-invocation via `--allow-empty-changesets`) |

## `[release]`

Credentials and registry endpoints are deliberately excluded — this table only declares immutable intent slots; execution receives credentials from its caller.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `product-package` | string | required | Ecosystem-qualified package whose binaries ship as release assets, e.g. `cargo/callisto-cli` |
| `forge-repository` | string | none | `owner/repo` the product release publishes to |
| `[[release.artifact]]` | array of tables | `[]` | Declared binary artifacts (see below) |
| `[release.profiles]` | table | none | Legacy; only `profiles.production` is read, to migrate its `forge-repository` into the top-level key. Conflicting values between the two are a hard error. |

### `[[release.artifact]]`

| Key | Type | Required | Meaning |
| --- | --- | --- | --- |
| `package` | string | yes | Ecosystem-qualified package that builds this artifact (need not be `product-package`) |
| `target` | string | yes | Opaque target triple — passed through to the builder, never parsed by Callisto |
| `asset-name` | string | yes | Exact GitHub release asset filename |

Example (this repo's own `callisto.toml`):
```toml
[release]
product-package = "cargo/callisto-cli"
forge-repository = "orin-dx/callisto"

[[release.artifact]]
package = "cargo/callisto-cli"
target = "aarch64-apple-darwin"
asset-name = "callisto-aarch64-apple-darwin.tar.gz"
```

## `[registries]`

Keyed by registry name. Two entries exist by default even with no `[registries]` table at all: `crates-io` (kind `cargo`) and `npm` (kind `npm`), both with no URL override.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `registries.<name>.kind` | string (`"cargo"` \| `"npm"` \| `"pypi"`) | `"npm"` if omitted | Registry ecosystem |
| `registries.<name>.url` | string | none | Registry endpoint override |

Example:
```toml
[registries.internal-npm]
kind = "npm"
url = "https://npm.internal.example.com"
```

## `[[package]]` and `[[package-set]]`

Both blocks share five override fields, parsed identically. `[[package]]`'s `match` is an exact `PackageId` (first matching rule wins, prefixed forms like `cargo/foo` take priority over bare `foo` regardless of declaration order). `[[package-set]]`'s `match` is a glob `PackagePattern` and can match many packages at once; `[[package]]` always takes priority over `[[package-set]]` for a given package.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `match` | string | required | `[[package]]`: exact ecosystem-qualified id or bare name. `[[package-set]]`: glob pattern. |
| `release-trigger` | string (`"changeset"` \| `"auto"`) | none — every field here is an `Option`; omitted means "use this package's own default", not a config-level default | What causes this package to be considered for release |
| `publish-to` | array of strings (`"crates-io"` \| `"npm"` \| `"pypi"` \| `"nuget"` \| `"github-release"` \| `"none"`) | none | Where this package publishes |
| `tag-template` | string | none | Git tag template, e.g. `"callisto@{version}"` |
| `previous-tag-templates` | array of strings | `[]` | Earlier tag templates checked (in order) for this package's last release tag, only if `tag-template` finds none |
| `changelog` | string | none | Changelog path, relative to the package's own root. Forward-slash-separated only; rejected if absolute or containing `..` (E113). |
| `pre-major-inference` | string (`"off"` \| `"conservative"` \| `"conservative-feat"`) | `"off"` | Pre-1.0 severity-downgrade policy: `conservative` downgrades an inferred Major to Minor; `conservative-feat` also downgrades Minor to Patch |

Example (this repo's own `callisto.toml`):
```toml
[[package]]
match = "cargo/callisto-cli"
publish-to = ["crates-io", "github-release"]
tag-template = "callisto@{version}"
previous-tag-templates = ["callisto-cli@{version}"]
```

## `[[fixed-group]]` and `[[linked-group]]`

Neither has a config-level default for its fields — both are required when the block is present.

| Key | Type | Required | Meaning |
| --- | --- | --- | --- |
| `name` | string | yes | Group identifier, unique across both fixed and linked groups |
| `members` | array of strings | yes, non-empty | Package names in the group |

`[[fixed-group]]`: all members bump in lock-step to the maximum version required across the group. `[[linked-group]]`: members share severity but keep independent version numbers.

Example (this repo's own `callisto.toml`):
```toml
[[fixed-group]]
name = "workspace"
members = ["callisto-model", "callisto-format", "callisto-graph", "callisto-cli"]
```

## `[init]`

Deprecated. `ecosystems` (array of strings) is parsed so older config files still load, but nothing reads it.

## Could not fully verify

- `registries.<name>.kind` defaulting to `"npm"` when omitted (not just when the whole `[registries]` table is absent) is read directly from `resolve.rs`'s `match` arm (`Some("npm") | None => Ecosystem::Npm`) — confirmed, not a guess, but worth a second look since defaulting an *explicitly present but keyless* registry entry to npm is a slightly surprising choice.
- `release-trigger`/`publish-to`/`tag-template`/`pre-major-inference` at `[[package]]`/`[[package-set]]` level are all `Option<T>` with no config-level default — the doc comment says "use the package's default" but where that fallback default is actually computed lives in the graph engine's package-construction path, not in `config/resolve.rs`. Not traced further; flagged for whoever owns that code path.

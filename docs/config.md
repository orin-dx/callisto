# Configuration reference

Every key `callisto.toml` accepts, from `crates/callisto-graph/src/config/{raw,resolve,groups}.rs`. Unknown keys are rejected (`#[serde(deny_unknown_fields)]`).

This reference describes current keys, not the [proposed canonical-identity/polyglot design](architecture/polyglot-v1.md). Exact references can currently be bare or qualified; prefer `cargo/name`, `npm/@scope/name` and `pypi/name` when writing new configuration. No release-owner, generic build-input or per-file PyPI configuration is implemented yet.

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
| `registries.<name>.url` | string | none | Registry endpoint override. For `pypi` it is the simple index Callisto reads, and the upload endpoint unless `publish-url` is set |
| `registries.<name>.publish-url` | string | `url` | `pypi` only: the endpoint Twine uploads to when it differs from the index. Same HTTPS rules as `url` |

An explicit registry entry still defaults `kind` to npm when omitted; requiring it is planned. For `pypi`, set `publish-url` when the index and the upload endpoint differ, for example `url = "https://test.pypi.org/simple"` with `publish-url = "https://test.pypi.org/legacy/"`; see [Python support limits](publishing.md#current-python-support-limits). Accepting `nuget` as a target string does not provide a working NuGet publisher.

Example:
```toml
[registries.internal-npm]
kind = "npm"
url = "https://npm.internal.example.com"
```

## `[[package]]` and `[[package-set]]`

Both blocks share five override fields, parsed identically; `[[package]]` also accepts `previous-tag-templates`. `[[package]]`'s `match` is an exact ecosystem-qualified id (first matching rule wins). `[[package-set]]`'s `match` is an ecosystem-qualified glob and can match many packages at once; a rule for several ecosystems is one rule per ecosystem; `[[package]]` always takes priority over `[[package-set]]` for a given package.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `match` | string | required | `[[package]]`: exact `ecosystem/name`. `[[package-set]]`: `ecosystem/glob`. A name without its ecosystem is E317. |
| `release-trigger` | string (`"changeset"` \| `"auto"`) | none — every field here is an `Option`; omitted means "use this package's own default", not a config-level default | What causes this package to be considered for release |
| `publish-to` | array of strings (`"crates-io"` \| `"npm"` \| `"pypi"` \| `"nuget"` \| `"github-release"` \| `"none"`) | none | Where this package publishes |
| `tag-template` | string | none | Git tag template, e.g. `"callisto@{version}"` |
| `previous-tag-templates` | array of strings | `[]` | `[[package]]` only (`[[package-set]]` rejects it). Earlier tag templates checked (in order) for this package's last release tag, only if `tag-template` finds none. They are matched by name only, so this is how a renamed package claims the tags it made under its old name |
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
| `members` | array of strings | yes, non-empty | `ecosystem/name` of each package in the group. A platform package is named by its native name |

`[[fixed-group]]`: all members bump in lock-step from the group's alignment base. `[[linked-group]]`: every member receives the highest severity and aligns to the highest target version produced by the members' bumps. Both currently require compatible version grammars; neither is an automatic cross-language version converter.

Example (this repo's own `callisto.toml`):
```toml
[[fixed-group]]
name = "workspace"
members = ["cargo/callisto-model", "cargo/callisto-manifests", "cargo/callisto-graph", "cargo/callisto-cli"]
```

## `[init]`

Deprecated. `ecosystems` (array of strings) is parsed so older config files still load, but nothing reads it.

## Identity and defaults

Unspecified package fields are filled during graph construction: the release trigger defaults to changesets, the changelog to the package directory, and publish targets to those declared by its manifests. Default tags use the display id, so newly created tags become qualified after name promotion; historical bare tags use the ownership fallback described in [identity](architecture/identity.md). Canonical references alone do not repair the open rename/demotion failures.

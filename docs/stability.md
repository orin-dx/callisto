# Stability

What Callisto promises to keep working from 1.0, and what it does not. A breaking change to anything in the first table needs a major version.

## Stable

| Surface | The promise |
| --- | --- |
| The eight commands (`add`, `status`, `version`, `pre`, `snapshot`, `init`, `release`, `completions`) and their flags | A flag keeps its meaning; a new flag never changes what the command did without it. |
| Exit codes | `0` success, `1` failure, `2` usage error. |
| `--format json` reports | Every report carries `schemaVersion`. Within a version a field is not removed, renamed or retyped; fields may be added. A shape change bumps the version. |
| `callisto.toml` | A key keeps its meaning. Unknown keys are an error, so a new key arrives in a minor release and never changes an existing file's behavior. |
| Changesets | The `.changeset/*.md` format and `pre.json` stay compatible with `@changesets/cli` for npm names; other ecosystems use `ecosystem/name` keys. |
| Error codes | A code keeps naming the same condition and is never reused. New codes may appear. |
| Tag names | `{name}@{version}` by default, `ecosystem/name@{version}` once another ecosystem has a package of that name. Pin a name with `tag-template` if a stable tag matters to your tooling. |
| The GitHub Actions | The inputs of `callisto-action` and `setup-callisto`. The floating `v1` tag moves only on stable releases; pin a commit for a fixed version. |

## CI plumbing

The generated workflows use `release plan`, `release inspect`, `release execute`, `release artifact-manifest`, `release build-python`, `release-pr`, `matrix`, `compose-pr-body` and `schema`. They are hidden from `--help` and work only with the same Callisto version that generated them: the intent, decision, receipt and run envelope carry versions, and a reader rejects any other. Keep the action and the binary on one version, and regenerate with `callisto init` after an upgrade.

## Not covered

- Text output, stderr, warnings and the wording of error messages.
- The library crates (`callisto-model`, `-manifests`, `-graph`). They are internal and version in lockstep with the CLI.
- The order of keys in JSON output, and timing or performance.
- Platforms without a published binary. Windows is not supported.

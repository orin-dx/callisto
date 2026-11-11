---
callisto-cli: patch
---

`callisto add` no longer writes a changeset that Callisto's own tools reject, and `status` JSON has the same fields for every package.

- `add --package cargo/a:patch --package cargo/a:major` now fails with E329 and writes nothing. Before, it wrote both lines, and `status --check` then failed on the file.
- `status --format json`: `lastTag` and `pendingSeverity` print `null` when there is none instead of being left out, so every package record has the same keys.
- The `--strict` help for `snapshot` and `version` now says what the flag does: it turns warnings into errors, and graph errors abort either way.

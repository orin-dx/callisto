---
callisto-cli: patch
---

An empty changeset (the shape `@changesets/cli add --empty` writes) is now accepted instead of rejected; status reports an entries-empty changeset as advisory instead of an error, and reports each unparseable changeset as a per-file error instead of aborting the whole command

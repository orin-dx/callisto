---
callisto-cli: patch
---

schema --type validate is removed and --type pre is renamed to --type pre-state (the .changeset/pre.json file shape, not pre's own report); an unrecognized --type value is now a usage error listing the supported values

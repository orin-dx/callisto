---
callisto-cli: patch
---

**A failed `callisto init` can be retried**

When `init` cannot write `.changeset/README.md`, it now stops before writing `callisto.toml`, so running `callisto init` again once the directory is writable completes the setup instead of refusing an already initialized workspace.

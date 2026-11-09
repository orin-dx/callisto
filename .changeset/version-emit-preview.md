---
callisto-cli: minor
---

`callisto version --emit-preview <path>` writes a JSON file describing the release (packages, changes and warnings), and `compose-pr-body --preview <path>` writes the release PR body from it. A script that builds its own release PR can write the body after the changesets are consumed. Relative paths resolve against `--cwd`.

---
callisto-cli: minor
---

`callisto version --emit-preview <path>` saves the facts a release PR is built from, and `compose-pr-body --preview <path>` writes the PR body from that file, so a script can build the body after the changesets are consumed. Relative paths resolve against `--cwd`.

---
callisto-cli: minor
---

`callisto version --emit-preview <path>` writes the facts a release PR body is built from, and `compose-pr-body --preview <path>` renders from that file. The release PR action uses both, so the body can be composed after the changesets are consumed. `--emit-preview` with `--dry-run` fails with E276, as `--emit-decision` does.

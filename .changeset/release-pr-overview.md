---
callisto-cli: minor
---

The release PR tells you what to expect before you merge.

**What you see**
- Changes grouped under Major, Minor and Patch, each titled by its first line instead of its file name, with long notes folded away
- A "Read before merging" block listing `Breaking:` and `Action:` notes, a `Breaking:` note whose bump is too low, and packages that changed but are not in this release
- Where merging will publish each package, with a warning if the release would be refused
- Labels such as `callisto: ready`, `blocked` and `bump: minor`, so you can triage without opening the PR
  - Missing labels are created, and labels that stop applying are removed

**Scripting**
- `callisto version --emit-preview <path>` writes the release as JSON
- `compose-pr-body --preview <path>` builds the PR body from that file
  - Relative paths resolve against `--cwd`
- `compose-pr-body --publish-plan` adds the publish section; the release PR action already passes it

**Upgrading**
- Nothing to change. Start a changeset paragraph with `Breaking:` or `Action:` to have it listed.

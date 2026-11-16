---
callisto-cli: patch
---

A changelog that already has a title such as `# Changelog` no longer ends up with two titles, and front matter stays on top.

- Before, the first release written to a `CHANGELOG.md` titled `# Changelog` (or anything other than the package name) added a second `# <package>` title above it. A file that started with `---` front matter got the new title above the front matter, which stopped it being front matter.
- Now the leading title is replaced by the package title, and front matter stays first.
- You do not need to do anything. Changelog sections already written are not changed.

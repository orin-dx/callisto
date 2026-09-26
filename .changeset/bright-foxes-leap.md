---
callisto-cli: patch
---

callisto version now refreshes npm, pnpm, yarn, bun and pdm lockfiles by default (in addition to Cargo, uv and poetry), opt out with --no-refresh-lockfiles; a failed refresh now stops the command with a coded error naming the lockfile and command

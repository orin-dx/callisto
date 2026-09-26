---
callisto-cli: patch
---

Workspace discovery no longer silently drops or misattaches packages: a manifest that fails to parse now fails with a clear error naming the path when it's a real workspace member, and warns otherwise; root detection asks git itself instead of hand-walking for .git, so GIT_DIR and worktrees are respected, and it reads [workspace]/[package] as real TOML tables instead of matching the text (a commented-out mention no longer counts); a platform package sitting beside its own Cargo.toml or pyproject.toml is never misattached to an unrelated owner; and a [[package]] rule that matches no packages gets a warning, matching the existing [[package-set]] one.

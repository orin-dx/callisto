---
callisto-cli: patch
---

**Large pnpm workspaces load much faster.** Relative `workspace:../path` dependencies are now resolved through one directory index instead of a scan per dependency.

- At 800 packages with relative `workspace:` paths, `status` drops from about 4.9 s to 0.6 s, `version --dry-run` from 7.6 s to 0.8 s and `release --dry-run` from 6.3 s to 0.9 s.
- No configuration change is needed. Dependency edges and warnings are unchanged.

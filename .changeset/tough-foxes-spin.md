---
callisto-cli: patch
---

Callisto's GitHub release workflow can finish an interrupted release after `main` advances.

- It resumes from the merged release PR's commit.
- It finishes older pending releases before preparing the next release PR.

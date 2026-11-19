---
callisto-cli: patch
---

**An interrupted `version` run resumes only with its own workspace's plan.** Running `version` again after an interruption now refuses with `E340` if the plan would come from any directory other than this workspace's recorded tree, instead of writing another workspace's bump, notes and changelog here.

- Delete the checkpoint ref named in the error and follow the `E121` steps, as for any refused resume.

# git ls-remote

Captured 2026-09-19 with git 2.55.0 by `testing/refresh-provider-fixtures.sh` against the public `https://github.com/orin-dx/callisto.git`. Each file is the unmodified stdout of `git ls-remote <remote> refs/tags/<tag> 'refs/tags/<tag>^{}'`, the exact shape the tag provider issues.

- `ls-remote-annotated.txt`: `callisto-changelog@0.3.1`, an annotated tag: the tag object line plus the peeled `^{}` commit line.
- `ls-remote-lightweight.txt`: `callisto-vcs@0.2.0`, a lightweight tag: one line, no peeled line.
- `ls-remote-absent.txt`: `callisto-no-such-tag`; empty output, exit 0.

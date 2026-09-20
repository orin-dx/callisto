# npm view fixtures

Captured 2026-09-20 with npm 11.16.0 by `testing/refresh-provider-fixtures.sh`.
Read-only queries against the public registry, no credentials.

Each `.cmd` file is the exit code, stdout and stderr of one real run, in the
envelope `exit: N`, `--- stdout`, `--- stderr`.

- `found.cmd`: `npm view left-pad@1.3.0 version --json`. Exit 0, stdout is the
  JSON version string.
- `missing.cmd`: `npm view callisto-no-such-package-zz@1.0.0 version --json`.
  Exit 1 with `npm error code E404`. npm's trailing "A complete log of this run
  can be found in ..." line is dropped by the refresh script because it names
  the capturing machine's home directory; everything else is as printed.

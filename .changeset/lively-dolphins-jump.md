---
callisto-cli: minor
---

**Dry-run refusals share one error code, and a no-op flag is gone.** `--dry-run` on a command that cannot preview now always fails with `E309`, naming the command.

- Why it matters: one code to match instead of five, and fewer codes to keep stable.
- How to adopt it: match `E309` where you matched `E216`, `E217`, `E218` or `E282`.
- Limits: the message and fix text now name the command, for example `release plan refuses --dry-run`.

**Breaking:** `callisto version --refresh-lockfiles` is removed. It did nothing, since lockfiles refresh by default. Remove the flag from scripts; use `--no-refresh-lockfiles` to skip the refresh.

**Breaking:** `E216`, `E217`, `E218` and `E282` are retired and no longer emitted. `release artifact-manifest`, `release plan`, `release execute` and `release-pr commit-plan` with `--dry-run` now fail with `E309`, as `release build-python`, `release pack-npm` and `release package-cargo` already did.

**Action:** update scripts that match `E216`, `E217`, `E218` or `E282` to match `E309`, and drop `--refresh-lockfiles` from `version` calls.

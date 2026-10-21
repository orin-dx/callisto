---
callisto-cli: minor
---

`version` and `snapshot` now stop with E319 before writing anything when a dependency range that consumers install would keep excluding the new version cannot be rewritten, such as a Cargo `>=1.2.0, <2.0.0` range after a major bump. It used to warn, release the dependent anyway and leave an unresolvable range. An npm range with a prerelease clause, such as `^2.0.0-next.0` left over from a prerelease cycle, is now rewritten like any other (`^3.0.0`). Dev dependencies still only warn.

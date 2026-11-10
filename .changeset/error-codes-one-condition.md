---
callisto-cli: minor
---

Error codes now each name one condition, so a script can tell problems apart from the code.

- `callisto.toml`: E111 is only a TOML syntax error. An unknown key is E325, and a value of the wrong type or a missing required key is E326.
- `release pending --from`: a value that is not a full 40-character SHA is E327, and a valid SHA that is not a release commit on this branch is E328. Before, both showed E171, which means an internal defect.

Breaking: scripts that match E111 to detect an unknown key or a wrong-type value, or E171 to detect a bad `--from`, must match E325, E326, E327 or E328 instead.

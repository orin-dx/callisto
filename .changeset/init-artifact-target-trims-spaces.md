---
callisto-cli: patch
---

`callisto init --artifact-target "a, b"` no longer writes a config that every later command rejects.

- Before, a space after the comma stayed in the target (`" aarch64-apple-darwin"`), and the next command failed with `E197`.
- Now the spaces are trimmed, and an empty entry such as `a,,b` is a usage error.
- You do not need to do anything. If `init` already wrote a target with a leading space, remove the space in `callisto.toml`.

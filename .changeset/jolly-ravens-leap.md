---
callisto-cli: minor
---

A release no longer stops with E157 when npm shows a published version late. After each publish Callisto now waits for the version to appear, up to 6 minutes for npm and PyPI and 2 for Cargo, instead of about 30 seconds, and the wait applies to a rerun that finds the version already published. A registry that is slower can set `confirm-timeout` on its `[registries]` entry in callisto.toml, for example `confirm-timeout = "10m"`.

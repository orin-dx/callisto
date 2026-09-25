---
callisto-cli: minor
---

**One release destination: `[release].forge-repository`**

The forge destination moves to `forge-repository = "owner/repo"` directly under `[release]`. Each publish target uses its own registry key (`[registries.<key>]` or the built-in default). A legacy `[release.profiles.production].forge-repository` is still read when the new key is absent.

Breaking:
- `--profile` is removed from `release plan` and `release execute`.
- `[release.profiles.<name>]` other than `production` is rejected; `registry-routes` is ignored.
- Setting both `[release].forge-repository` and a different `[release.profiles.production].forge-repository` is rejected.
- Release intents are schema v5 and run envelopes v3; re-plan intents made by older versions.

---
callisto-cli: minor
---

A release of several npm packages now waits for the registry once instead of once per package. Callisto publishes the npm packages back to back, a package after the dependency it follows without waiting for the registry to show that dependency, then looks at all of them together until each version is visible. Tags, GitHub releases and uploads still wait until every npm version they follow is shown, so nothing is announced early. A release of eight npm packages that took eight waits of up to six minutes now takes one. Cargo and PyPI publishes are still confirmed one at a time.

---
callisto-cli: patch
---

Bumping a version in `package.json` now changes only that line. Before, it also rewrote every inline array and object in the file (`"files": ["dist"]` became four lines), turning a one-line release into a noisy diff.

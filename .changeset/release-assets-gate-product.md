---
callisto-cli: patch
---

`callisto release` publishes a product's GitHub release only after every configured asset has uploaded, including assets built by other packages, and uploads use the product's prerelease flag. Before, such releases could publish incomplete or fail with E167.

---
callisto-cli: minor
---

**Release artifacts are declared per package**

Any targets and asset names are now accepted, not only Callisto's own four. Each artifact is recorded under the package that builds it; one whose package isn't in the release fails with E179. `--package` on one `[[fixed-group]]` member releases the whole group.

Breaking: replace `[release] artifact-targets` with one block per target:

```toml
[[release.artifact]]
package = "cargo/my-cli"
target = "aarch64-apple-darwin"
asset-name = "my-cli-aarch64-apple-darwin.tar.gz"
```

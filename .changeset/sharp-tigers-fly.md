---
callisto-cli: minor
---

Release binaries are now published for Intel macOS (`x86_64-apple-darwin`) and arm64 Linux (`aarch64-unknown-linux-gnu`) as well as the existing three, and the `setup-callisto` action downloads them on those machines instead of building from crates.io. An older release without the new asset still installs from crates.io.

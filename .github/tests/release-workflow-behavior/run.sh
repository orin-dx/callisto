#!/usr/bin/env bash
# Exercise the release CLI with process-boundary provider fakes. The Rust
# acceptance tests create real temporary Git repositories and assert provider
# observations, rather than parsing workflow YAML as a substitute for runtime
# behavior.
set -euo pipefail

cargo test -p callisto-cli --test durable_release_e2e_tests

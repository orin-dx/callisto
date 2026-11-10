#!/usr/bin/env bash
# Exercises the release CLI against process-boundary provider fakes in real temporary Git repositories.
set -euo pipefail

cargo test -p callisto-cli --test durable_release_e2e_tests

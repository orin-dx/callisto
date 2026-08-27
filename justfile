# Default recipe: fast local CI pipeline (no coverage) via moon & just.
# Use `just ci` explicitly for full CI parity including coverage.
default: ci-fast

# Build debug workspace binaries via moon
build:
    moon run :build

# Build release CLI binary
build-release:
    cargo build --release -p callisto-cli

# Run unit, integration, doctests, and E2E tests
test:
    cargo nextest run --workspace || moon run :test
    cargo test --doc

# Run Clippy lints (warnings treated as errors) as a single workspace invocation.
# Moon's per-project `cargo clippy -p $project` tasks all lock the same shared
# target/ dir, so running them one-per-project serializes on Cargo's own build
# lock instead of parallelizing — a single --workspace invocation lets Cargo's
# internal job scheduler parallelize across crates instead.
lint:
    cargo clippy --workspace --all-targets -- -D warnings

# Lint only projects affected by changes since the base branch. Uses moon's
# project graph to find affected crates, then lints them in one cargo
# invocation (not moon's per-project fan-out) to avoid Cargo's build-lock
# serialization while still skipping unaffected crates entirely.
lint-affected:
    #!/usr/bin/env bash
    set -euo pipefail
    projects=$(moon query projects --affected 2>/dev/null | jq -r '.projects[].id')
    if [ -z "$projects" ]; then
        echo "No projects affected — skipping lint."
        exit 0
    fi
    args=()
    for p in $projects; do args+=(-p "$p"); done
    cargo clippy "${args[@]}" --all-targets -- -D warnings

# Check code formatting compliance via moon
fmt-check:
    moon run :format-check

# Format code automatically via moon
fmt:
    moon run :format

# Security advisory & license check via moon / cargo-deny
audit:
    moon run :audit

# Run mutation testing sweep via cargo-mutants
mutants:
    cargo mutants --workspace

# Run unused dependency check via cargo-machete
machete:
    cargo machete

# Verify public Rust API SemVer breaking changes
check-api:
    cargo semver-checks check-release

# Run structure-aware fuzzing target
fuzz target="parse_package_id":
    cargo fuzz run {{target}}

# Documentation build check (warnings treated as errors)
doc-check:
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace

# Verify Moon WASM plugin cross-compilation target and build the real
# cdylib. The black-box Extism/wasmtime sandbox test (tests/moon_wasm_sandbox.rs)
# is NOT re-run here: `just test`'s workspace-wide nextest run already builds
# and executes it (moon_pdk_test_utils is an unconditional dev-dependency, not
# gated by the `pdk` feature), so running it again via `cargo test` here would
# be a third full execution of the same suite once `coverage` runs it a second
# time under instrumentation -- pure redundant runtime, not extra coverage.
wasm-check:
    rustup target add wasm32-wasip1 2>/dev/null || true
    cargo check -p callisto-moon --target wasm32-wasip1 --features pdk
    cargo rustc -p callisto-moon --lib --target wasm32-wasip1 --features pdk --crate-type cdylib

# Generate code coverage report via cargo-llvm-cov. `_pdk.rs`-suffixed files
# are excluded: they contain code that only executes inside a real
# wasm32-wasip1 Extism host (see e.g. crates/callisto-moon/src/runner_pdk.rs's
# module doc comment) -- black-box tested via tests/moon_wasm_sandbox.rs, but
# invisible to native coverage instrumentation by construction, not a real
# testing gap. Any file matching this naming convention is understood to
# document its own exclusion this way.
#
# Optional `threshold`: when set, fails if total line coverage drops below
# it (--fail-under-lines). Unset locally (informational only, matching
# ARCHITECTURE.md's "coverage generation is a CI-only gate" note); CI calls
# `just coverage 90` -- this is the one command both run, so a CI coverage
# failure always reproduces locally with the exact same invocation.
coverage threshold="":
    #!/usr/bin/env bash
    set -euo pipefail
    args=(--all-features --lcov --output-path lcov.info --ignore-filename-regex '_pdk\.rs$')
    if [[ -n "{{threshold}}" ]]; then
      args+=(--fail-under-lines "{{threshold}}")
    fi
    cargo llvm-cov "${args[@]}"

# Check per-crate line coverage against a threshold (default 90%). The
# workspace-total --fail-under-lines gate can pass while a small crate is
# far below threshold, since a few large crates dominate the total line
# count -- this catches that case. Requires profile data from a prior
# `cargo llvm-cov` run in this session (e.g. `just coverage` just ran, or
# CI's own coverage step ran first); does not re-run tests itself.
coverage-per-crate threshold="90":
    #!/usr/bin/env bash
    set -euo pipefail
    cargo llvm-cov report --json --summary-only --ignore-filename-regex '_pdk\.rs$' > /tmp/callisto-cov-summary.json
    report=$(jq -r '
        .data[0].files[]
        | select(.filename | test("/crates/"))
        | {crate: (.filename | capture("/crates/(?<c>[^/]+)/").c), count: .summary.lines.count, covered: .summary.lines.covered}
        | [.crate, .count, .covered]
        | @tsv
      ' /tmp/callisto-cov-summary.json \
      | awk -F'\t' -v threshold="{{threshold}}" '
          {sum[$1]+=$2; cov[$1]+=$3}
          END {
            for (c in sum) {
              pct = (cov[c]/sum[c]*100)
              status = (pct+0 < threshold+0) ? "FAIL" : "pass"
              printf "%s\t%d\t%d\t%.2f\t%s\n", c, sum[c], cov[c], pct, status
            }
          }
        ' | sort)
    echo "$report" | awk -F'\t' '{printf "%-25s lines=%-6s covered=%-6s cover=%6s%%  %s\n", $1, $2, $3, $4, $5}'
    echo "$report" | grep -q FAIL && exit 1 || exit 0

# Clean build targets, Moon task caches, and generated artifacts
clean:
    moon clean 2>/dev/null || true
    cargo clean
    rm -f lcov.info callisto-schema.json

# Fast pre-commit hook validation (<500ms formatting check)
pre-commit: fmt-check

# Pre-push hook validation: formatting check plus clippy on affected projects only
pre-push: fmt-check lint-affected

# Install native Git pre-commit and pre-push hooks
hooks:
    @echo '#!/bin/sh\njust pre-commit' > .git/hooks/pre-commit
    @chmod +x .git/hooks/pre-commit
    @echo '#!/bin/sh\njust pre-push' > .git/hooks/pre-push
    @chmod +x .git/hooks/pre-push
    @echo "Git pre-commit and pre-push hooks installed successfully."

# Run full local CI verification pipeline via moon and just
ci: fmt-check lint test audit doc-check wasm-check coverage

# Same as `ci`, minus `coverage`. Real CI (callisto-ci.yml) already runs
# coverage as its own parallel job on a separate runner; locally it's a
# third full-workspace recompile under llvm-cov instrumentation tacked onto
# the end of a serial pipeline. Use this for everyday pre-PR checks;
# coverage numbers are a reporting concern, not something every local run
# needs to regenerate.
ci-fast: fmt-check lint test audit doc-check wasm-check

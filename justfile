# Default recipe: fast local CI pipeline (no coverage) via moon & just.
# Use `just ci` explicitly for full CI parity including coverage.
default: ci

# Build debug workspace binaries via moon
build:
    moon run :build

# Build release CLI binary
build-release:
    cargo build --release -p callisto-cli

# Prebuilds the wasm moon_wasm_sandbox.rs loads, so test processes don't race to build it.
build-moon-wasm:
    cargo rustc -p callisto-moon --lib --target wasm32-wasip1 --features pdk --crate-type cdylib

# Run unit, integration, doctests, and E2E tests. This intentionally uses the
# same Nextest command as CI: a runner/configuration failure must never be
# mistaken for a passing Moon fallback.
test: build-moon-wasm
    cargo nextest run --workspace --all-features
    cargo test --doc --all-features

# Explicit compatibility path for contributors who do not have cargo-nextest
# installed. It is never used by CI or pre-merge verification.
test-moon:
    moon run :test
    cargo test --doc --all-features

# Exercise the Release-PR action's real shell block with Git and GitHub API
# boundaries faked. This proves the script's control flow given a decision
# shape; it does not call the real `callisto` binary, so it cannot catch a
# mismatch between what Callisto actually serializes and what this script's
# jq expressions expect. See test-release-action-binary for that check.
test-release-action:
    bash .github/actions/callisto-action/tests/test_release_pr_contract.sh

# Compiles callisto and runs the Release-PR action's jq extraction against
# its real JSON output. Requires pending changesets to exercise update and
# supersede decisions; skips cleanly otherwise.
test-release-action-binary:
    bash .github/actions/callisto-action/tests/test_release_pr_decide_binary_contract.sh

# Compiles and runs the callisto-cli test suite under the shipped binary's
# default features only (no --all-features, no inference), so a bug reachable
# only in that build is caught here instead of only under --all-features.
test-ci-default-features:
    cargo nextest run -p callisto-cli
    cargo test --doc -p callisto-cli

# Execute the release lifecycle at the real CLI boundary with fake registry,
# Git, forge, and attestation providers. This is deliberately separate from
# broad workspace tests so PR CI makes the release behavior evidence visible.
release-workflow-behavior:
    bash .github/tests/release-workflow-behavior/run.sh

# Re-captures the real provider responses under testing/fixtures/providers with
# read-only network GETs. Manual: review the diff and update PROVENANCE.md.
provider-fixtures:
    bash testing/refresh-provider-fixtures.sh

# Manual and network-dependent, so deliberately not part of `ci`: re-fetches the
# live providers and asserts each fixture's SHAPE (status code, JSON key paths,
# ls-remote line kinds) still matches; content is never compared.
provider-contract:
    bash testing/refresh-provider-fixtures.sh --check

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

# Security advisory, ban, license & source check via cargo-deny (workspace root, one pass)
audit:
    cargo deny check

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
# it (--fail-under-lines). The human-readable summary is always emitted
# before enforcing the threshold: LCOV output itself contains no aggregate
# percentage, and a failed CI gate must say what developers need to improve.
# Unset means informational only; `just ci` and CI both call `just coverage 90`,
# so a CI coverage failure always reproduces locally with the same invocation.
coverage threshold="": build-moon-wasm
    #!/usr/bin/env bash
    set -euo pipefail
    args=(--all-features --lcov --output-path lcov.info --ignore-filename-regex '_pdk\.rs$')
    cargo llvm-cov "${args[@]}"
    cargo llvm-cov report --summary-only --ignore-filename-regex '_pdk\.rs$'
    if [[ -n "{{threshold}}" ]]; then
      cargo llvm-cov report --summary-only --ignore-filename-regex '_pdk\.rs$' --fail-under-lines "{{threshold}}"
    fi

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

# Static security audit of workflows and actions (requires zizmor and ripgrep).
zizmor:
    zizmor --offline --strict-collection .

# Credential-free release workflow contract, policy, and policy mutant checks.
release-workflow-checks:
    bash .github/tests/verify-release-workflow-contract.sh
    bash .github/tests/verify-release-workflow-policy.sh
    bash .github/tests/verify-release-workflow-policy.sh --self-test

# Workflow Contracts CI job minus actionlint and zizmor: pins, release
# contract/policy, artifact build script, the Release-PR action contract
# under a minimal PATH, and the installer verification-mode tests.
workflow-contracts: release-workflow-checks
    bash .github/tests/verify-action-pins.sh
    bash .github/tests/test-release-artifact-build-script.sh
    env PATH=/usr/bin:/bin bash .github/actions/callisto-action/tests/test_release_pr_contract.sh
    env PATH=/usr/bin:/bin bash .github/actions/callisto-action/tests/test_release_contract.sh
    bash .github/actions/setup-callisto/tests/test_download_extraction_format.sh
    bash .github/actions/setup-callisto/tests/test_crates_io_fallback.sh
    bash .github/actions/setup-callisto-wasm/tests/test_verification_modes.sh

# Every check CI runs except actionlint (Docker) and the binary-dependent
# release-PR decide contract and artifact preflight build. CI runs them as
# parallel jobs (callisto-ci.yml); locally they run in sequence.
ci: fmt-check lint test test-ci-default-features audit doc-check wasm-check zizmor workflow-contracts release-workflow-behavior (coverage "90")

# Default recipe: fast local CI pipeline (no coverage) via moon & just.
# Use `just ci` explicitly for full CI parity including coverage.
default: ci

# Build debug workspace binaries via moon
build:
    moon run :build

# Build release CLI binary
build-release:
    cargo build --release -p callisto-cli

# Time `status`, `version --dry-run` and `release --dry-run` over generated workspaces and count their git processes.
# `CALLISTO_BENCH_PACKAGES=50,200` picks the sizes.
bench:
    cargo bench -p callisto-cli --bench commands

# Run unit, integration, doctests, and E2E tests. This intentionally uses the
# same Nextest command as CI: a runner/configuration failure must never be
# mistaken for a passing Moon fallback.
test:
    cargo nextest run --workspace
    cargo test --doc --workspace

# Explicit compatibility path for contributors who do not have cargo-nextest
# installed. It is never used by CI or pre-merge verification.
test-moon:
    moon run :test
    cargo test --doc --workspace

# Exercise the Release-PR action's shell block with Git and GitHub faked; it never calls the real binary (see test-release-action-binary).
test-release-action:
    bash .github/actions/callisto-action/tests/test_release_pr_contract.sh

# Compiles callisto and runs the Release-PR action's jq extraction against
# its real JSON output. Requires pending changesets to exercise update and
# supersede decisions; skips cleanly otherwise.
test-release-action-binary:
    bash .github/actions/callisto-action/tests/test_release_pr_decide_binary_contract.sh

# Compiles callisto and feeds its real error output through the CI problem matcher.
test-problem-matcher:
    bash .github/tests/test-problem-matcher.sh

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

# Reject history words and plain comment blocks over three lines (`comment-block-ok` marks an exception).
check-comments:
    bash testing/check-comments.sh

# Clippy as one workspace invocation: moon's per-project runs serialize on cargo's shared target/ lock.
lint: check-comments
    cargo clippy --workspace --all-targets -- -D warnings
    cargo clippy --workspace --lib --bins -- -D warnings -D clippy::unwrap_used

# Lint only moon-affected projects, in one cargo invocation to avoid the build-lock serialization.
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
    cargo clippy "${args[@]}" --lib --bins -- -D warnings -D clippy::unwrap_used

# Check code formatting compliance via moon
fmt-check:
    moon run :format-check

# Format code automatically via moon
fmt:
    moon run :format

# Security advisory, ban, license & source check via cargo-deny (workspace root, one pass)
audit: license-boundary
    cargo deny check

# callisto-model is MIT: nothing it depends on at build or run time, directly or not, may be FSL-licensed or a Callisto FSL crate.
license-boundary:
    ! cargo tree -p callisto-model -e normal,build --prefix none --format '{p} {l}' | grep -E 'FSL|callisto-(manifests|graph|cli|fixtures)'

# Compile the fuzz targets so they cannot rot (running them needs nightly and cargo-fuzz)
fuzz-check:
    cargo check --manifest-path fuzz/Cargo.toml --bins

# Run mutation testing sweep via cargo-mutants
mutants:
    cargo mutants --workspace

# The shipped binary is built with the features its crates default to, and none of them is the dev-only `test-util`.
features-boundary:
    ! cargo tree -p callisto-cli -e normal,build -f '{p} {f}' | grep 'test-util'

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

# Coverage via cargo-llvm-cov; a `threshold` fails below that line coverage, after printing the summary.
# `just ci` and CI both call `just coverage 90`, so a CI failure reproduces locally.
coverage threshold="":
    #!/usr/bin/env bash
    set -euo pipefail
    args=(--workspace --lcov --output-path lcov.info)
    cargo llvm-cov "${args[@]}"
    cargo llvm-cov report --summary-only
    if [[ -n "{{threshold}}" ]]; then
      cargo llvm-cov report --summary-only --fail-under-lines "{{threshold}}"
    fi

# Per-crate line coverage against a threshold (default 90%): the workspace total can pass while a small crate is far below.
# Needs profile data from a prior `cargo llvm-cov` run; it does not re-run tests.
coverage-per-crate threshold="90":
    #!/usr/bin/env bash
    set -euo pipefail
    cargo llvm-cov report --json --summary-only > /tmp/callisto-cov-summary.json
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

# Workflow Contracts CI job minus actionlint and zizmor: pins, release
# contract/policy, artifact build script, the Release-PR action contract
# under a minimal PATH, and the installer verification-mode tests.
workflow-contracts: release-workflow-checks
    bash .github/tests/test_release_candidate_recovery.sh
    bash .github/tests/verify-action-pins.sh
    bash .github/tests/verify-action-inputs.sh
    bash .github/tests/test-release-artifact-build-script.sh
    bash .github/tests/test-move-floating-tag.sh
    bash .github/tests/test-hold-release-pr.sh
    env PATH=/usr/bin:/bin bash .github/actions/callisto-action/tests/test_release_pr_contract.sh
    env PATH=/usr/bin:/bin bash .github/actions/callisto-action/tests/test_release_contract.sh
    bash .github/actions/setup-callisto/tests/test_download_extraction_format.sh
    bash .github/actions/setup-callisto/tests/test_action_ref_version_resolution.sh
    bash .github/actions/setup-callisto/tests/test_crates_io_fallback.sh

# Every check CI runs except actionlint (Docker) and the binary-dependent
# release-PR decide contract, problem matcher contract and artifact preflight build. CI runs them as
# parallel jobs (callisto-ci.yml); locally they run in sequence.
ci: fmt-check lint fuzz-check test audit doc-check zizmor workflow-contracts release-workflow-behavior (coverage "90")

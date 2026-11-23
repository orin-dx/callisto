set shell := ["bash", "-euo", "pipefail", "-c"]

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
    #!/usr/bin/env bash
    set -euo pipefail
    tree=$(cargo tree -p callisto-model -e normal,build --prefix none --format '{p} {l}')
    if [ -z "$tree" ]; then echo "cargo tree printed nothing" >&2; exit 1; fi
    if grep -E 'FSL|callisto-(manifests|graph|cli|fixtures)' <<<"$tree"; then exit 1; fi

# Compile the fuzz targets so they cannot rot (running them needs nightly and cargo-fuzz)
fuzz-check:
    cargo check --manifest-path fuzz/Cargo.toml --bins

# Run mutation testing sweep via cargo-mutants
mutants:
    cargo mutants --workspace

# The shipped binary is built with the features its crates default to, and none of them is the dev-only `test-util`.
features-boundary:
    #!/usr/bin/env bash
    set -euo pipefail
    tree=$(cargo tree -p callisto-cli -e normal,build -f '{p} {f}')
    if [ -z "$tree" ]; then echo "cargo tree printed nothing" >&2; exit 1; fi
    if grep 'test-util' <<<"$tree"; then exit 1; fi

# The local npm, PyPI and Cargo registries the real-registry suites run against, and the Changesets 2.x and 3.x CLIs
# the compatibility differential runs against, installed under target/real-tools.
# gunicorn: pypiserver's default backend does a slow getfqdn() bind. setuptools and wheel: the offline Python build.
install-real-tools:
    #!/usr/bin/env bash
    set -euo pipefail
    for tool in npm uv; do
        command -v "$tool" >/dev/null || { echo "$tool is required: the install matrix uses npm, pnpm and uv" >&2; exit 1; }
    done
    tools=target/real-tools
    mkdir -p "$tools"
    export COREPACK_HOME="$PWD/$tools/corepack/home" COREPACK_ENABLE_AUTO_PIN=0
    npm install --silent --prefer-offline --no-audit --no-fund --prefix "$tools/npm" verdaccio@6.10.4 pnpm@11.9.0 @changesets/cli@3.0.3 bun@1.4.5
    # Its own prefix: the corepack package ships pnpm and npm shims that would replace the pinned pnpm.
    npm install --silent --prefer-offline --no-audit --no-fund --prefix "$tools/corepack-cli" corepack@0.36.0
    mkdir -p "$tools/corepack/bin"
    "$tools/corepack-cli/node_modules/.bin/corepack" enable --install-directory "$tools/corepack/bin" yarn
    "$tools/corepack-cli/node_modules/.bin/corepack" prepare yarn@4.18.1 --activate
    "$tools/corepack/bin/yarn" --version
    "$tools/npm/node_modules/.bin/bun" --version
    npm install --silent --prefer-offline --no-audit --no-fund --prefix "$tools/changesets-2" @changesets/cli@2.31.1
    PATH="$PWD/$tools/npm/node_modules/.bin:$PATH" pnpm --version
    PATH="$PWD/$tools/npm/node_modules/.bin:$PATH" changeset --version
    "$tools/changesets-2/node_modules/.bin/changeset" --version
    uv venv -q --seed --allow-existing --python 3.12 "$tools/venv"
    python_pins="pypiserver==2.4.0 twine==6.2.0 build==1.4.4 legacy-cgi==2.6.3 gunicorn==26.2.0 setuptools==80.9.0 wheel==0.45.1"
    "$tools/venv/bin/python" -m pip install -q $python_pins
    "$tools/venv/bin/python" -m pip download -q -d "$tools/wheels" setuptools==80.9.0 wheel==0.45.1
    cargo install --locked --root "$tools/cargo" cargo-http-registry@0.1.8

# Rehearses recovery against local npm, PyPI and Cargo registries, then runs the native install matrix.
# `native=no` skips the native resolution suite, which `just test` already runs.
test-real-tools native="yes": install-real-tools
    #!/usr/bin/env bash
    set -euo pipefail
    tools="$PWD/target/real-tools"
    export PATH="$tools/corepack/bin:$tools/venv/bin:$tools/npm/node_modules/.bin:$tools/cargo/bin:$PATH"
    export COREPACK_HOME="$tools/corepack/home" COREPACK_ENABLE_AUTO_PIN=0 COREPACK_ENABLE_DOWNLOAD_PROMPT=0
    cargo --version && npm --version && pnpm --version && uv --version && yarn --version && bun --version
    REAL_TOOLS_CHANGESETS_2="$tools/changesets-2/node_modules/.bin/changeset" REAL_TOOLS_WHEELS="$tools/wheels" cargo test -p callisto-cli --test release_verify_real -- --ignored --test-threads=1
    [ "{{native}}" = no ] || cargo test -p callisto-cli --test native_resolution_e2e_tests -- --test-threads=1

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
    summary=$(mktemp)
    trap 'rm -f "$summary"' EXIT
    cargo llvm-cov report --json --summary-only > "$summary"
    report=$(jq -r '
        .data[0].files[]
        | select(.filename | test("/crates/"))
        | {crate: (.filename | capture("/crates/(?<c>[^/]+)/").c), count: .summary.lines.count, covered: .summary.lines.covered}
        | [.crate, .count, .covered]
        | @tsv
      ' "$summary" \
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
    if [ -z "$report" ]; then echo "no per-crate coverage data: run cargo llvm-cov first" >&2; exit 1; fi
    echo "$report" | awk -F'\t' '{printf "%-25s lines=%-6s covered=%-6s cover=%6s%%  %s\n", $1, $2, $3, $4, $5}'
    if grep -q FAIL <<<"$report"; then exit 1; fi

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
    @printf '#!/bin/sh\njust pre-commit\n' > .git/hooks/pre-commit
    @chmod +x .git/hooks/pre-commit
    @printf '#!/bin/sh\njust pre-push\n' > .git/hooks/pre-push
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

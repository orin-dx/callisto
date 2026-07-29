# Default recipe: run full CI pipeline via moon & just
default: ci

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

# Run Clippy lints (warnings treated as errors) via moon
lint:
    moon run :lint

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

# Verify Moon WASM plugin cross-compilation target
wasm-check:
    rustup target add wasm32-wasip1 2>/dev/null || true
    cargo check -p callisto-moon --target wasm32-wasip1 --features pdk

# Generate code coverage report via cargo-llvm-cov
coverage:
    cargo llvm-cov --all-features --lcov --output-path lcov.info

# Clean build targets, Moon task caches, and generated artifacts
clean:
    moon clean 2>/dev/null || true
    cargo clean
    rm -f lcov.info callisto-schema.json

# Fast pre-commit hook validation (<500ms formatting check)
pre-commit: fmt-check

# Fast pre-push hook validation (<2s formatting and clippy lints check)
pre-push: fmt-check lint

# Install native Git pre-commit and pre-push hooks
hooks:
    @echo '#!/bin/sh\njust pre-commit' > .git/hooks/pre-commit
    @chmod +x .git/hooks/pre-commit
    @echo '#!/bin/sh\njust pre-push' > .git/hooks/pre-push
    @chmod +x .git/hooks/pre-push
    @echo "Git pre-commit and pre-push hooks installed successfully."

# Run full local CI verification pipeline via moon and just
ci: fmt-check lint test audit doc-check wasm-check coverage

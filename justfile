# Default recipe: run full CI pipeline via moon & just
default: ci

# Build debug workspace binaries via moon
build:
    moon run :build

# Build release CLI binary
build-release:
    cargo build --release -p callisto-cli

# Run unit, integration, doctests, and E2E tests via moon
test:
    moon run :test
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

# Security advisory check via moon
audit:
    moon run :audit

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

# Run full local CI verification pipeline via moon and just
ci: fmt-check lint test audit doc-check wasm-check coverage

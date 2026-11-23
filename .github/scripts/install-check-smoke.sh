#!/bin/sh
# Install-check smoke test: the binary reports the expected version and runs `status` on a fixture workspace.
# Usage: install-check-smoke.sh <callisto-binary> <expected-version>
set -eu

callisto=$1
expected=$2

actual=$("$callisto" --version)
echo "$actual"
if [ "$actual" != "callisto $expected" ]; then
  echo "::error::expected callisto $expected, got $actual"
  exit 1
fi

fixture=$(mktemp -d)
mkdir -p "$fixture/crates/a/src"
cd "$fixture"
printf '[workspace]\nmembers = ["crates/a"]\nresolver = "3"\n' > Cargo.toml
printf '[package]\nname = "a"\nversion = "0.1.0"\nedition = "2024"\n' > crates/a/Cargo.toml
echo 'fn main() {}' > crates/a/src/main.rs
git init -q
git add -A
git -c user.name=ci -c user.email=ci@example.com -c commit.gpgsign=false commit -qm init
"$callisto" status

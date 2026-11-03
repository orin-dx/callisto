#!/usr/bin/env bash
# The problem matcher must match what the real binary prints on failure, or CI gets no annotations.
# Feeds real stderr through the matcher's own regular expressions, one pattern per consecutive line.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
matcher="$root/.github/callisto-problem-matcher.json"

cargo build -p callisto-cli --quiet --manifest-path "$root/Cargo.toml"
bin="$root/target/debug/callisto"

work="$(mktemp -d "${TMPDIR:-/tmp}/callisto.XXXXXX")"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/crates/a/src"
printf '[workspace]\nmembers = ["crates/a"]\nresolver = "2"\n' > "$work/Cargo.toml"
printf '[package]\nname = "a"\nversion = "0.1.0"\nedition = "2021"\n' > "$work/crates/a/Cargo.toml"
: > "$work/crates/a/src/lib.rs"
git -C "$work" init -q

stderr="$(NO_COLOR=1 "$bin" --cwd "$work" matrix --package cargo/nope 2>&1 >/dev/null || true)"

python3 -I - "$matcher" "$stderr" <<'PY'
import json, re, sys

patterns = json.load(open(sys.argv[1]))["problemMatcher"][0]["pattern"]
lines = sys.argv[2].split("\n")
for start in range(len(lines)):
    window = lines[start:start + len(patterns)]
    if len(window) == len(patterns) and all(re.search(p["regexp"], l) for p, l in zip(patterns, window)):
        captured = {}
        for p, l in zip(patterns, window):
            m = re.search(p["regexp"], l)
            for key in ("code", "message"):
                if key in p:
                    captured[key] = m.group(p[key])
        if captured.get("code") == "E102" and "cargo/nope" in captured.get("message", ""):
            print("PASS: the problem matcher matches the binary's real error:", captured)
            sys.exit(0)
print("FAIL: the problem matcher does not match the real error output:\n" + sys.argv[2])
sys.exit(1)
PY

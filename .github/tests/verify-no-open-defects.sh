#!/usr/bin/env bash
# Fails while any parked defect test (#[ignore = "DEFECT-...") remains.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

if matches=$(grep -rn -F --include='*.rs' '#[ignore = "DEFECT-' crates); then
  printf 'open defects remain:\n%s\n' "$matches" >&2
  exit 1
fi
echo "no open defects"

#!/usr/bin/env bash
# Regression test for AC-002: the NATIVE_MATRIX computation's
# unique_by(.artifactName) dedup (action.yml, in the branch after
# "hasChangesets=false") must retain both packages' entries when two
# packages declare the same triple, now that artifactName embeds
# package_name (SPEC-002 AC-001) and is therefore distinct per package.
# Extracts the exact jq filter text out of action.yml so this test cannot
# drift from the file's current real logic.
set -u
ACTION_YML="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/action.yml"

FILTER_LINE=$(grep -F 'unique_by(.artifactName)' "$ACTION_YML")
if [[ -z "$FILTER_LINE" ]]; then
  echo "FAIL: no unique_by(.artifactName) line found in action.yml"
  exit 1
fi

FILTER=$(echo "$FILTER_LINE" | sed -n "s/.*jq -c '\\(.*\\)'.*/\\1/p")
if [[ -z "$FILTER" ]]; then
  echo "FAIL: could not extract jq filter from: $FILTER_LINE"
  exit 1
fi

fail=0

# Fixture mirrors callisto matrix --format json's shape post-AC-001: two
# packages declaring the same triple now have distinct artifactName values.
FIXTURE='{"platformTargets":{"pkg-a":{"targets":[{"artifactName":"native-pkg-a-x86_64-apple-darwin","packageDir":"pkg-a"}]},"pkg-b":{"targets":[{"artifactName":"native-pkg-b-x86_64-apple-darwin","packageDir":"pkg-b"}]}}}'
OUT=$(echo "$FIXTURE" | jq -c "$FILTER")
COUNT=$(echo "$OUT" | jq 'length')
if [[ "$COUNT" -ne 2 ]]; then
  echo "FAIL AC-002 (distinct artifactName retained): expected 2 entries, got $COUNT: $OUT"; fail=1
else
  echo "PASS AC-002 (distinct artifactName retained)"
fi

exit $fail

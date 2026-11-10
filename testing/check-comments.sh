#!/usr/bin/env bash
# Rejects history words in comments and plain comment blocks longer than three lines (rustdoc `///` and `//!` are exempt from the cap).
# A block that needs more carries `comment-block-ok` on one of its lines.
set -euo pipefail

cd "$(dirname "$0")/.."

history='previously|originally|no longer|before the fix|audit pattern|(`|[)]|[^a-z](it|they|which|that|this|we)) used to'
max_block=3

files=$(git ls-files 'crates/*.rs' 'fuzz/*.rs' '.github/*.sh' '.github/*.yml' 'testing/*.sh' justfile)

# shellcheck disable=SC2086
awk -v history="$history" -v max="$max_block" '
function flush() {
    if (len > max && !ok) {
        printf "%s:%d: comment block of %d lines (limit %d); cut it or mark it comment-block-ok\n", start_file, start, len, max
        bad = 1
    }
    len = 0; ok = 0
}
FNR == 1 { flush() }
{
    if (FNR == 1) rust = (FILENAME ~ /\.rs$/)
    line = $0
    is_comment = rust ? (line ~ /^[ \t]*\/\//) : (line ~ /^[ \t]*#/ && line !~ /^#!/)
    is_doc = rust && (line ~ /^[ \t]*\/\/[\/!]/)
    if (is_comment) {
        if (!is_doc) {
            if (len == 0) { start = FNR; start_file = FILENAME }
            len++
            if (line ~ /comment-block-ok/) ok = 1
        } else flush()
        scan = tolower(line)
        gsub(/no longer than/, "", scan)
        if (scan ~ history) {
            printf "%s:%d: comment narrates history: %s\n", FILENAME, FNR, line
            bad = 1
        }
    } else {
        flush()
    }
}
END { flush(); exit bad }
' $files

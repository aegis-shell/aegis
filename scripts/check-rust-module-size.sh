#!/usr/bin/env bash
set -euo pipefail

# A deliberately generous tripwire: crossing it requires an architectural
# split, not an allowlist entry. Smaller files may still need review when they
# mix responsibilities; this guard only prevents known monoliths returning.
limit="${AEGIS_MAX_RUST_MODULE_LINES:-2000}"
status=0

while IFS= read -r file; do
    lines="$(wc -l < "$file")"
    if (( lines > limit )); then
        echo "$file: $lines lines exceeds the $limit-line module limit" >&2
        status=1
    fi
done < <(rg --files crates -g '*.rs')

exit "$status"

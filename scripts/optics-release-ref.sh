#!/bin/sh
set -eu

manifest=${1:-Cargo.toml}
expected_packages=7

if [ ! -f "$manifest" ]; then
    printf 'error: manifest not found: %s\n' "$manifest" >&2
    exit 1
fi

refs=$(
    sed -n \
        's/^[a-zA-Z0-9_-]* = { git = "https:\/\/github.com\/ming2k\/optics", tag = "\([^"]*\)" }$/\1/p' \
        "$manifest"
)
package_count=$(printf '%s\n' "$refs" | sed '/^$/d' | wc -l)
unique_refs=$(printf '%s\n' "$refs" | sed '/^$/d' | sort -u)
unique_count=$(printf '%s\n' "$unique_refs" | sed '/^$/d' | wc -l)

if [ "$package_count" -ne "$expected_packages" ]; then
    printf 'error: expected %s tagged Optics dependencies in %s, found %s\n' \
        "$expected_packages" "$manifest" "$package_count" >&2
    exit 1
fi

if [ "$unique_count" -ne 1 ]; then
    printf 'error: Optics dependencies in %s do not use one release tag\n' \
        "$manifest" >&2
    exit 1
fi

printf '%s\n' "$unique_refs"

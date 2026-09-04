#!/bin/sh
set -eu

cargo test -p tessera-launcher --no-run

test_binary=$(
    find target/debug/deps -maxdepth 1 -type f -name 'tessera_launcher-*' -perm -u+x \
        -printf '%T@ %p\n' \
        | sort -nr \
        | sed -n '1s/^[^ ]* //p'
)

if [ -z "$test_binary" ]; then
    printf '%s\n' 'could not locate the tessera-launcher unit-test binary' >&2
    exit 1
fi

unit="tessera-interaction-domain-test-$$"
exec systemd-run --user --wait --pipe --collect \
    --unit="$unit" \
    --property='Delegate=cpu memory pids' \
    "$test_binary" --test-threads=1 "$@"

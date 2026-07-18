#!/bin/sh
set -eu

cargo test -p ass-launch --no-run

test_binary=$(
    find target/debug/deps -maxdepth 1 -type f -name 'ass_launch-*' -perm -u+x \
        -printf '%T@ %p\n' \
        | sort -nr \
        | sed -n '1s/^[^ ]* //p'
)

if [ -z "$test_binary" ]; then
    printf '%s\n' 'could not locate the ass-launch unit-test binary' >&2
    exit 1
fi

unit="ass-realm-test-$$"
exec systemd-run --user --wait --pipe --collect \
    --unit="$unit" \
    --property='Delegate=cpu memory pids' \
    "$test_binary" --test-threads=1 "$@"

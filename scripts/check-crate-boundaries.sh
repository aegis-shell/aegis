#!/usr/bin/env bash
set -euo pipefail

metadata=$(cargo metadata --format-version 1 --no-deps "$@")

internal_dependencies() {
  local package=$1
  jq -r --arg package "$package" '
    .packages[]
    | select(.name == $package)
    | .dependencies[]
    | select(.kind == null or .kind == "normal")
    | select(.name == "tessera" or (.name | startswith("tessera-")))
    | .name
  ' <<<"$metadata" | sort -u
}

assert_internal_dependencies() {
  local package=$1
  shift
  local allowed=" $* "
  local dependency
  local failed=false

  while IFS= read -r dependency; do
    [[ -z "$dependency" ]] && continue
    if [[ "$allowed" != *" $dependency "* ]]; then
      printf '%s must not depend on %s\n' "$package" "$dependency" >&2
      failed=true
    fi
  done < <(internal_dependencies "$package")

  if [[ "$failed" == true ]]; then
    return 1
  fi
}

# Foundation crates stay effect-free or transport-neutral. Higher layers may
# depend downward, but server, renderer, shell, and binary crates never leak
# back into these contracts.
assert_internal_dependencies tessera-model
assert_internal_dependencies tessera-wayland-protocols
assert_internal_dependencies tessera-security tessera-model
assert_internal_dependencies tessera-semantic tessera-model
assert_internal_dependencies tessera-config tessera-model
assert_internal_dependencies tessera-ipc tessera-model tessera-security tessera-semantic
assert_internal_dependencies tessera-ipc-client tessera-model tessera-ipc

# Native management commands remain independently buildable and testable.
# tessera-security is allowed for the local durable-audit operations (ADR-0137):
# `tessera audit status|verify|ack-export` open the sealed audit store directly,
# never through a running compositor.
assert_internal_dependencies tessera-commands tessera-model tessera-config tessera-ipc tessera-security

# Core compositor mechanisms do not depend on one another through the binary
# or through presentation-layer crates.
assert_internal_dependencies tessera-backend tessera-model tessera-wayland-protocols
assert_internal_dependencies tessera-render tessera-model
assert_internal_dependencies \
  tessera-compositor tessera-model tessera-semantic tessera-wayland-protocols

printf 'crate dependency boundaries: ok\n'

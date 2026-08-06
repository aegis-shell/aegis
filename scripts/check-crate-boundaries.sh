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
    | select(.name == "aegis" or (.name | startswith("aegis-")))
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
assert_internal_dependencies aegis-model
assert_internal_dependencies aegis-wayland-protocols
assert_internal_dependencies aegis-security aegis-model
assert_internal_dependencies aegis-semantic aegis-model
assert_internal_dependencies aegis-config aegis-model
assert_internal_dependencies aegis-ipc aegis-model aegis-security aegis-semantic

# Native management commands remain independently buildable and testable.
assert_internal_dependencies aegis-commands aegis-model aegis-config aegis-ipc

# Core compositor mechanisms do not depend on one another through the binary
# or through presentation-layer crates.
assert_internal_dependencies aegis-backend aegis-model aegis-wayland-protocols
assert_internal_dependencies aegis-render aegis-model
assert_internal_dependencies \
  aegis-compositor aegis-model aegis-semantic aegis-wayland-protocols

printf 'crate dependency boundaries: ok\n'

#!/usr/bin/env bash
set -Eeuo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
repo_root=$(cd -- "$script_dir/.." && pwd -P)
scratch=$(mktemp -d)

cleanup() {
    rm -rf -- "$scratch"
}
trap cleanup EXIT INT TERM

mkdir -p \
    "$scratch/bin" \
    "$scratch/runtime" \
    "$scratch/target/debug" \
    "$scratch/target/release"

cat >"$scratch/bin/fake-cargo" <<'EOF'
#!/usr/bin/env bash
set -eu
printf 'cargo %s\n' "$*" >>"$AEGIS_DEV_TEST_LOG"
EOF

for profile in debug release; do
    cat >"$scratch/target/$profile/aegis" <<'EOF'
#!/usr/bin/env bash
set -eu

printf 'start %s\n' "$*" >>"$AEGIS_DEV_TEST_LOG"
printf 'runtime %s\n' "$XDG_RUNTIME_DIR" >>"$AEGIS_DEV_TEST_LOG"
printf 'path %s\n' "$PATH" >>"$AEGIS_DEV_TEST_LOG"
printf 'data %s\n' "$XDG_DATA_DIRS" >>"$AEGIS_DEV_TEST_LOG"
printf 'display %s\n' "${WAYLAND_DISPLAY-unset}" >>"$AEGIS_DEV_TEST_LOG"
EOF
    cat >"$scratch/target/$profile/aegis-settings" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
done

chmod +x \
    "$scratch/bin/fake-cargo" \
    "$scratch/target/debug/aegis" \
    "$scratch/target/debug/aegis-settings" \
    "$scratch/target/release/aegis" \
    "$scratch/target/release/aegis-settings"

export AEGIS_DEV_CARGO=$scratch/bin/fake-cargo
export AEGIS_DEV_SKIP_DISPLAY_CHECK=1
export AEGIS_DEV_TEST_LOG=$scratch/dev.log
export CARGO_TARGET_DIR=$scratch/target
export XDG_RUNTIME_DIR=$scratch/runtime
export WAYLAND_DISPLAY=wayland-test

"$repo_root/scripts/dev.sh"

stage_dir=$scratch/target/aegis-dev
if ! grep -q '^cargo build --package aegis --bin aegis --package aegis-settings --bin aegis-settings$' \
    "$AEGIS_DEV_TEST_LOG"
then
    printf '%s\n' 'dev.sh did not build the integrated debug session' >&2
    exit 1
fi
if ! grep -q '^start --backend nested$' "$AEGIS_DEV_TEST_LOG"; then
    printf '%s\n' 'dev.sh did not select the safe nested default' >&2
    exit 1
fi
if [[ ! -x $stage_dir/bin/aegis-settings ]] ||
    [[ ! -f $stage_dir/share/applications/io.github.ming2k.aegis.Settings.desktop ]] ||
    [[ ! -f $stage_dir/share/icons/hicolor/scalable/apps/io.github.ming2k.aegis.Settings.svg ]]
then
    printf '%s\n' 'dev.sh did not stage the complete Settings application' >&2
    exit 1
fi
if ! grep -q "^path $stage_dir/bin:" "$AEGIS_DEV_TEST_LOG" ||
    ! grep -q "^data $stage_dir/share:" "$AEGIS_DEV_TEST_LOG"
then
    printf '%s\n' 'dev.sh did not publish the staged application prefix' >&2
    exit 1
fi
nested_runtime=$(sed -n 's/^runtime //p' "$AEGIS_DEV_TEST_LOG" | head -n 1)
if [[ $nested_runtime == "$XDG_RUNTIME_DIR" || -e $nested_runtime ]]; then
    printf '%s\n' 'dev.sh did not clean its isolated nested runtime' >&2
    exit 1
fi

: >"$AEGIS_DEV_TEST_LOG"
"$repo_root/scripts/dev.sh" --backend drm --release --no-build

if grep -q '^cargo ' "$AEGIS_DEV_TEST_LOG"; then
    printf '%s\n' 'dev.sh --no-build invoked Cargo' >&2
    exit 1
fi
if ! grep -q '^start --backend drm$' "$AEGIS_DEV_TEST_LOG" ||
    ! grep -q '^display unset$' "$AEGIS_DEV_TEST_LOG"
then
    printf '%s\n' 'dev.sh did not start the explicit DRM environment' >&2
    exit 1
fi

if "$repo_root/scripts/dev.sh" --backend auto \
    >"$scratch/invalid.stdout" 2>"$scratch/invalid.stderr"
then
    printf '%s\n' 'dev.sh accepted the unsafe auto backend' >&2
    exit 1
fi

printf '%s\n' 'development workflow: ok'

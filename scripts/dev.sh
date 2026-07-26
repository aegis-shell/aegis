#!/usr/bin/env bash
set -Eeuo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
repo_root=$(cd -- "$script_dir/.." && pwd -P)

cargo_command=${AEGIS_DEV_CARGO:-cargo}

backend=nested
build=true
profile=debug
app_args=()

target_dir=
stage_dir=
binary=
dev_runtime=
host_display=

usage() {
    cat <<'EOF'
Usage: scripts/dev.sh [OPTIONS] [-- AEGIS_ARGS...]

Build, stage, and run an integrated Aegis development session. Nested mode
is the safe default. Direct DRM/KMS access must be selected explicitly.

Options:
  --backend nested|drm  Presentation backend (default: nested)
  --release             Build and run release binaries
  --no-build            Use already-built binaries for a one-shot session
  -h, --help            Show this help

Arguments after -- are passed to Aegis. Do not pass --backend there.
EOF
}

die() {
    printf 'aegis-dev: %s\n' "$*" >&2
    exit 2
}

log() {
    printf 'aegis-dev: %s\n' "$*"
}

command_exists() {
    command -v -- "$1" >/dev/null 2>&1
}

parse_args() {
    while (($# > 0)); do
        case $1 in
            --backend)
                (($# >= 2)) || die "--backend requires nested or drm"
                backend=$2
                shift 2
                ;;
            --backend=*)
                backend=${1#*=}
                shift
                ;;
            --release)
                profile=release
                shift
                ;;
            --no-build)
                build=false
                shift
                ;;
            -h | --help)
                usage
                exit 0
                ;;
            --)
                shift
                app_args=("$@")
                break
                ;;
            *)
                die "unknown option '$1'; try --help"
                ;;
        esac
    done

    case $backend in
        nested | drm) ;;
        *) die "--backend requires nested or drm, got '$backend'" ;;
    esac
    for argument in "${app_args[@]}"; do
        case $argument in
            --backend | --backend=*)
                die "select the backend with dev.sh's --backend option"
                ;;
        esac
    done
}

resolve_paths() {
    target_dir=${CARGO_TARGET_DIR:-$repo_root/target}
    if [[ $target_dir != /* ]]; then
        target_dir=$repo_root/$target_dir
    fi
    target_dir=$(readlink -m -- "$target_dir")
    stage_dir=$target_dir/aegis-dev
    binary=$target_dir/$profile/aegis
}

validate_runtime() {
    local runtime=${XDG_RUNTIME_DIR:-}

    [[ -n $runtime ]] || die "\$XDG_RUNTIME_DIR is unset; log in through PAM/logind"
    [[ -d $runtime ]] || die "\$XDG_RUNTIME_DIR is not a directory: $runtime"
    [[ -O $runtime ]] || die "\$XDG_RUNTIME_DIR is not owned by the current user: $runtime"
    [[ -w $runtime && -x $runtime ]] ||
        die "\$XDG_RUNTIME_DIR is not writable and searchable: $runtime"
}

resolve_host_display() {
    local display=${WAYLAND_DISPLAY:-}

    [[ -n $display ]] || die "\$WAYLAND_DISPLAY is unset; nested mode needs a Wayland session"
    if [[ $display = /* ]]; then
        host_display=$display
    else
        host_display=$XDG_RUNTIME_DIR/$display
    fi
    if [[ ${AEGIS_DEV_SKIP_DISPLAY_CHECK:-0} != 1 && ! -S $host_display ]]; then
        die "host Wayland socket not found: $host_display"
    fi
}

prepare_nested_runtime() {
    local outer_runtime=$XDG_RUNTIME_DIR
    local entry

    dev_runtime=$(mktemp -d --tmpdir="$outer_runtime" aegis-dev.XXXXXX)
    chmod 700 "$dev_runtime"
    for entry in bus pipewire-0 pipewire-0-manager pulse at-spi dconf; do
        if [[ -e $outer_runtime/$entry || -L $outer_runtime/$entry ]]; then
            ln -s -- "$outer_runtime/$entry" "$dev_runtime/$entry"
        fi
    done
    log "runtime directory: $dev_runtime"
}

cleanup() {
    local status=$?
    trap - EXIT INT TERM

    if [[ -n $dev_runtime ]]; then
        case $dev_runtime in
            "${XDG_RUNTIME_DIR:?}"/aegis-dev.*)
                rm -rf -- "$dev_runtime"
                ;;
            *)
                printf 'aegis-dev: refusing to remove unexpected runtime path: %s\n' \
                    "$dev_runtime" >&2
                ;;
        esac
    fi
    exit "$status"
}

stage_apps() {
    local settings_binary=$target_dir/$profile/aegis-settings
    local desktop_source=$repo_root/contrib/io.github.ming2k.aegis.Settings.desktop
    local icon_source=$repo_root/contrib/icons/hicolor/scalable/apps/io.github.ming2k.aegis.Settings.svg

    [[ -x $settings_binary ]] ||
        die "aegis-settings binary is missing or not executable: $settings_binary"
    [[ -f $desktop_source ]] || die "desktop file is missing: $desktop_source"
    [[ -f $icon_source ]] || die "application icon is missing: $icon_source"

    install -d -m 0755 \
        "$stage_dir/bin" \
        "$stage_dir/share/applications" \
        "$stage_dir/share/icons/hicolor/scalable/apps"
    install -m 0755 "$settings_binary" "$stage_dir/bin/aegis-settings"
    install -m 0644 "$desktop_source" \
        "$stage_dir/share/applications/io.github.ming2k.aegis.Settings.desktop"
    install -m 0644 "$icon_source" \
        "$stage_dir/share/icons/hicolor/scalable/apps/io.github.ming2k.aegis.Settings.svg"
}

build_and_stage() {
    local -a cargo_profile=()

    [[ $profile == release ]] && cargo_profile=(--release)
    log "building Aegis and first-party applications ($profile)"
    "$cargo_command" build "${cargo_profile[@]}" \
        --package aegis \
        --bin aegis \
        --package aegis-settings \
        --bin aegis-settings || return 1
    log "staging first-party applications at $stage_dir"
    stage_apps
}

run_one() {
    local dev_path=$stage_dir/bin:${PATH:-/usr/local/bin:/usr/bin:/bin}
    local dev_data_dirs=$stage_dir/share:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}

    [[ -x $binary ]] || die "Aegis binary is missing or not executable: $binary"
    if [[ $backend == nested ]]; then
        log "starting one nested development session"
        XDG_RUNTIME_DIR=$dev_runtime \
            WAYLAND_DISPLAY=$host_display \
            PATH=$dev_path \
            XDG_DATA_DIRS=$dev_data_dirs \
            "$binary" --backend nested "${app_args[@]}"
    else
        log "starting one DRM development session"
        env -u WAYLAND_DISPLAY \
            PATH=$dev_path \
            XDG_DATA_DIRS=$dev_data_dirs \
            "$binary" --backend drm "${app_args[@]}"
    fi
}

main() {
    parse_args "$@"
    resolve_paths
    validate_runtime

    if [[ $backend == nested ]]; then
        resolve_host_display
        prepare_nested_runtime
    elif [[ $(id -u) == 0 ]]; then
        die "do not run the DRM backend as root; use a logind or seatd user session"
    fi

    if [[ $build == true ]]; then
        command_exists "$cargo_command" ||
            die "required command not found: $cargo_command"
    fi
    if [[ $build == true ]]; then
        build_and_stage
    else
        log "staging already-built first-party applications at $stage_dir"
        stage_apps
    fi
    run_one
}

trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

main "$@"

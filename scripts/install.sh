#!/usr/bin/env bash
set -Eeuo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
repo_root=$(cd -- "$script_dir/.." && pwd -P)

cargo_command=${AEGIS_CARGO:-cargo}
build=true
profile=release
install_mode=user
prefix=

usage() {
    cat <<'EOF'
Usage: scripts/install.sh [OPTIONS]

Build and install Aegis binaries, systemd units, desktop entries, and portal configuration.

Options:
  --user               Install to user directory ($HOME/.local, default)
  --prefix PREFIX      Install to specified prefix (e.g., /usr/local)
  --debug              Build and install debug binaries instead of release
  --no-build           Skip cargo build step and install existing binaries
  -h, --help           Show this help message
EOF
}

die() {
    printf 'aegis-install: %s\n' "$*" >&2
    exit 2
}

log() {
    printf 'aegis-install: %s\n' "$*"
}

parse_args() {
    while (($# > 0)); do
        case $1 in
            --user)
                install_mode=user
                shift
                ;;
            --prefix)
                (($# >= 2)) || die "--prefix requires a path argument"
                install_mode=prefix
                prefix=$2
                shift 2
                ;;
            --prefix=*)
                install_mode=prefix
                prefix=${1#*=}
                shift
                ;;
            --debug)
                profile=debug
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
            *)
                die "unknown option '$1'; try --help"
                ;;
        esac
    done
}

resolve_destinations() {
    if [[ $install_mode == user ]]; then
        local user_base=${XDG_DATA_HOME:-$HOME/.local/share}
        bin_dir=$HOME/.local/bin
        applications_dir=$user_base/applications
        icons_dir=$user_base/icons/hicolor/scalable/apps
        systemd_dir=${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user
        dbus_services_dir=$user_base/dbus-1/services
        portals_dir=$user_base/xdg-desktop-portal/portals
    else
        [[ -n $prefix ]] || die "prefix path must not be empty"
        bin_dir=$prefix/bin
        applications_dir=$prefix/share/applications
        icons_dir=$prefix/share/icons/hicolor/scalable/apps
        systemd_dir=$prefix/lib/systemd/user
        dbus_services_dir=$prefix/share/dbus-1/services
        portals_dir=$prefix/share/xdg-desktop-portal/portals
    fi
}

build_workspace() {
    local -a cargo_flags=()
    [[ $profile == release ]] && cargo_flags=(--release)

    log "building workspace binaries ($profile profile)"
    "$cargo_command" build "${cargo_flags[@]}" --workspace
}

install_artifacts() {
    local target_dir=${CARGO_TARGET_DIR:-$repo_root/target}
    if [[ $target_dir != /* ]]; then
        target_dir=$repo_root/$target_dir
    fi
    local build_dir=$target_dir/$profile

    log "installing binaries to $bin_dir"
    install -d -m 0755 "$bin_dir"
    for bin in aegis aegis-settings aegis-ctl aegis-fuji-mcp; do
        if [[ -x $build_dir/$bin ]]; then
            install -m 0755 "$build_dir/$bin" "$bin_dir/$bin"
            log "  installed $bin"
        fi
    done

    log "installing desktop entry to $applications_dir"
    install -d -m 0755 "$applications_dir"
    install -m 0644 "$repo_root/contrib/io.github.ming2k.aegis.Settings.desktop" \
        "$applications_dir/io.github.ming2k.aegis.Settings.desktop"

    log "installing application icon to $icons_dir"
    install -d -m 0755 "$icons_dir"
    install -m 0644 "$repo_root/contrib/icons/hicolor/scalable/apps/io.github.ming2k.aegis.Settings.svg" \
        "$icons_dir/io.github.ming2k.aegis.Settings.svg"

    log "installing systemd user unit to $systemd_dir"
    install -d -m 0755 "$systemd_dir"
    install -m 0644 "$repo_root/contrib/systemd/user/aegis.service" \
        "$systemd_dir/aegis.service"

    log "installing D-Bus portal service to $dbus_services_dir"
    install -d -m 0755 "$dbus_services_dir"
    install -m 0644 "$repo_root/contrib/dbus-1/services/org.freedesktop.impl.portal.desktop.aegis.service" \
        "$dbus_services_dir/org.freedesktop.impl.portal.desktop.aegis.service"

    log "installing XDG portal definition to $portals_dir"
    install -d -m 0755 "$portals_dir"
    install -m 0644 "$repo_root/contrib/xdg-desktop-portal/portals/aegis.portal" \
        "$portals_dir/aegis.portal"
}

main() {
    parse_args "$@"
    resolve_destinations

    if [[ $build == true ]]; then
        build_workspace
    fi

    install_artifacts
    log "Aegis installation completed successfully!"
}

main "$@"

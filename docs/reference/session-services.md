# Session Service Commands

`aegis` supervises `aegis-idle`, and `aegis-idle` starts `aegis-lock`.
Installed session binaries must be siblings. The session path does not search
`PATH` for replacements.

## Lock Screen

| Command or option | Result |
|-------------------|--------|
| `aegis-lock` | Lock the Wayland session connected by `WAYLAND_DISPLAY`. |
| `aegis-lock --ready-fd FD` | Write one byte after the compositor confirms that the secure frame was presented on every output. `FD` must be an inherited, writable descriptor. |
| `aegis-lock --help` | Print local usage without connecting to Wayland. |
| `aegis-lock --version` | Print the package version. |

The command exits with status `0` after authenticated unlock. A setup,
protocol, or rendering failure exits nonzero. An unavailable authentication
service leaves the secure lock active and displays an error. If a confirmed
lock client disappears, the compositor keeps the session fail-closed;
restarting a different locker cannot take over that lock.

## Idle Coordinator

| Command or option | Default | Result |
|-------------------|---------|--------|
| `aegis-idle` | — | Run one idle coordinator for the current session. |
| `--lock-now` | off | Ask the running coordinator to lock; start the sibling lock screen directly if the coordinator is unavailable. |
| `--dim-after SECONDS\|off` | `300` | Set or disable the backlight-dim stage. |
| `--lock-after SECONDS\|off` | `600` | Set or disable the automatic lock stage. |
| `--display-off-after SECONDS\|off` | `660` | Set or disable physical display power-off. |
| `--suspend-after SECONDS\|off` | `1800` | Set or disable the logind suspend request. |
| `--dim-percent 1..100` | `30` | Set the dimmed hardware-backlight percentage. |
| `--socket PATH` | `$XDG_RUNTIME_DIR/aegis.sock` | Select the compositor IPC socket used for output power. |
| `--control-socket PATH` | `$XDG_RUNTIME_DIR/aegis-idle.sock` | Select the owner-only datagram control socket. |
| `--no-logind` | off | Disable lock-before-sleep monitoring, the sleep delay inhibitor, and automatic suspend. |
| `--help` | — | Print local usage. |
| `--version` | — | Print the package version. |

Timeouts are seconds, must be strictly increasing when enabled, and cannot
exceed `604800`. Display-off and suspend require an enabled lock stage.
Invalid policy or a second live coordinator exits nonzero.

The compositor supplies validated options from `[idle]`; manual daemon
invocation is primarily for diagnostics. Nested sessions automatically
disable backlight dimming, physical output power, logind integration, and
suspend while retaining the lock stage.

See the [Configuration Reference](config.md#idle-and-locking) for persistent
policy, [How to Install and Verify the Lock Screen](../how-to/lock-screen.md)
for installation and PAM validation, and
[How to Configure Locking and Idle](../how-to/lock-and-idle.md) for the user
workflow.

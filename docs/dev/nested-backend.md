# Nested Backend Development

Use the nested backend for the default compositor development workflow. It runs
aegis inside an existing Wayland desktop, so crashes and restarts affect one
host window instead of the active VT. Use the DRM/KMS backend only for
behavior that depends on direct hardware or session ownership.

Complete the [development setup](setup.md) before using this workflow. The
architectural reason for keeping both backends behind one interface is covered
by [ADR-0003](../adr/0003-nested-first-bring-up.md).

## Runtime Model

The nested process is both a Wayland client and a Wayland compositor:

```text
outer Wayland compositor
└── aegis xdg-toplevel window (outer Wayland client)
    ├── Vulkan swapchain and aegis chrome
    ├── aegis Wayland server socket (wayland-N)
    └── inner Wayland clients
```

At startup, aegis connects to the inherited `$WAYLAND_DISPLAY`, creates an
`xdg_toplevel`, and presents Flux frames through a Wayland Vulkan surface. It
also creates a separate, auto-named Wayland server socket for applications
managed inside aegis, then points its own process environment at that socket.
The D-Bus and systemd activation environments are left to the host session:
exporting the inner socket there would send the host's activated services to
the nested compositor.

The host compositor supplies window size, scale, pointer, keyboard, gestures,
and text input. The nested backend converts those events into the same backend
interface used by DRM/KMS. The Wayland server, renderer, window manager, and
shell therefore run through their production paths; only presentation and raw
input ownership differ.

The nested window represents one output named `nested`. Resize the outer
window to exercise logical resize and swapchain recreation. Move it between
outputs to exercise integer or fractional scale changes supported by the host.

## Start One Instance

Run the following commands from the repository root in a terminal belonging
to an existing Wayland session:

```bash
scripts/dev.sh
```

The runner defaults to `--backend nested`; it never selects DRM implicitly.
It builds and stages the compositor and first-party applications, creates an
isolated runtime directory, and exits when that compositor instance exits.

A successful start opens an `aegis` window and logs both roles:

```text
flux: device created for nested backend; ...
server: listening on WAYLAND_DISPLAY=wayland-N
```

Close the outer window or press `Ctrl+C` in the terminal to stop the nested
session.

A direct `cargo run -p aegis -- --backend nested` starts only the compositor.
It does not stage first-party external applications. Use the runner when the
test includes System Settings discovery or launch.

## Run Applications Inside aegis

Prefer the built-in launcher. It passes the inner socket to the child process
explicitly, so launched applications reach the nested compositor regardless of
the environment they inherited.

System Settings is staged as an ordinary XDG application before the
compositor starts. Launch it from Applications to exercise binary discovery,
desktop metadata, icon resolution, window grouping, and IPC together. See
[First-Party Application Development](first-party-applications.md) for the
staging layout and focused workflows.

To start a test client from another terminal, copy the inner socket name from
the `server: listening` log line. When `scripts/dev.sh` is running, also copy
the private runtime directory printed by that command:

```bash
XDG_RUNTIME_DIR=/run/user/1000/aegis-dev.ABC123 \
WAYLAND_DISPLAY=wayland-1 \
foot
```

Replace both example values with the logged values. A client started without
these overrides connects to the outer desktop instead. For a manually started
non-isolated instance, keep the existing `$XDG_RUNTIME_DIR` and override only
`WAYLAND_DISPLAY`.

Run the in-tree IPC client against the development command's private runtime:

```bash
XDG_RUNTIME_DIR=/run/user/1000/aegis-dev.ABC123 \
    cargo run -p aegis-ctl -- windows
XDG_RUNTIME_DIR=/run/user/1000/aegis-dev.ABC123 \
    cargo run -p aegis-ctl -- outputs
```

Only one aegis IPC server can own an `aegis.sock` path. A manually started
nested process continues without IPC and logs an address-in-use warning when
the outer compositor already owns that path. Prefer `scripts/dev.sh`, which
creates and cleans a private nested runtime while preserving access to common
host-session services.

## Iterate on Changes

Treat configuration changes and code changes differently.

### Cargo Command Selection

Keep optimized release builds outside the normal edit-run loop. The workspace
development profile uses incremental compilation, 256 code-generation units,
light optimization for workspace crates, and moderate optimization for
dependencies. This keeps the compositor responsive while preserving reusable
development artifacts.

Choose the narrowest command that answers the current question:

| Goal | Command |
|------|---------|
| Check the crate being edited | `cargo check -p aegis-shell` |
| Check executable integration | `cargo check -p aegis` |
| Run one integrated nested instance | `scripts/dev.sh` |
| Run only the compositor binary | `cargo run -p aegis -- --backend nested` |
| Test one crate | `cargo test -p aegis-shell` |
| Validate the workspace | `cargo test --workspace` |
| Test the development workflow | `tests/dev-workflow.sh` |
| Build a production artifact | `cargo build --release -p aegis` |
| Profile a release build | `cargo build --release -p aegis --timings` |

Replace `aegis-shell` with the package being edited. Run the release commands
only before delivery, when validating production performance, or when a bug
depends on release optimization. The first build after changing the compiler,
profile, or features is a cold build and can still take several minutes;
subsequent builds reuse `target/debug` artifacts.

`--timings` writes the build report to
`target/cargo-timings/cargo-timing.html`. Use it before changing linker flags,
features, or crate boundaries so optimization work targets the measured
critical path.

### Configuration Changes

Edit `$XDG_CONFIG_HOME/aegis/config.toml`, defaulting to
`~/.config/aegis/config.toml`, while the nested session runs. The compositor
checks the file each frame and applies a valid change without restarting.
Invalid configuration leaves the previous configuration active and writes a
diagnostic to the log.

Wallpaper environment variables are read at process startup. Restart the
process after changing them. See the
[Configuration Reference](../reference/config.md) for the complete reload
contract.

### Rust Code Changes

Rust code is compiled into the executable loaded by the running process and is
not hot-reloaded in-process. Repeat this explicit development cycle:

1. Save the change.
2. Run `cargo check -p aegis` or wait for the editor's Rust check to pass.
3. Stop the nested process with `Ctrl+C`.
4. Run `scripts/dev.sh` again.
5. Relaunch any inner client needed for the test.

Each run recreates windows, Wayland connections, IPC state, and other
in-memory state.

## Choose the Validation Target

Use nested mode for the broad development loop, then run focused DRM/KMS
checks before completing work that crosses a backend boundary.

| Area | Nested backend | DRM/KMS backend |
|------|----------------|-----------------|
| Shell chrome, layout, animation, and window rules | Primary loop | Final smoke check |
| Wayland protocol and client-buffer behavior | Primary loop | Regression check when presentation-sensitive |
| Resize, host input forwarding, IME, and host scale changes | Primary loop | Not equivalent to device input |
| Atomic modesetting, page flips, and scanout formats | Not covered | Required |
| libinput devices and touchpad configuration | Not covered | Required |
| libseat ownership, VT suspend/resume, and hotplug | Not covered | Required |
| Software cursor on direct display | Not covered | Required |
| Realm cgroup delegation | Direct run is insufficient | Packaged systemd service required |

Use [VT/DRM Manual Testing](vt-drm-testing.md) when a change is ready to try on
real display and input hardware, especially after changes to DRM/KMS,
libinput, libseat, VT switching, or device hotplug.

## Troubleshooting

### The Host Display Cannot Be Opened

Confirm that the terminal belongs to a Wayland session and retains the outer
display variables:

```bash
printf '%s\n' "$WAYLAND_DISPLAY" "$XDG_RUNTIME_DIR"
```

Do not replace the shell's `$WAYLAND_DISPLAY` with the inner `wayland-N`
before starting aegis. The nested backend needs the outer display first.

### A Test Client Opens on the Outer Desktop

Start it with the inner socket logged by aegis, or start it through the built-in
launcher. The parent shell continues to point at the outer compositor by
design.

### Display or Touchpad Settings Are Read-Only

The outer compositor owns physical outputs and input devices. Nested mode can
observe its window size and scale, but it cannot modeset a monitor or configure
a libinput device. Validate those controls under DRM/KMS.

### Realm Launch Fails

A direct `cargo run` process does not own the delegated cgroup hierarchy that
Realm application sandboxes require. Use the packaged systemd service for
Realm launch tests, as described in the [development setup](setup.md).

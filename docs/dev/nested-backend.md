# Nested Backend Development

Use the nested backend for the default compositor development loop. It runs
ass inside an existing Wayland desktop, so crashes and restarts affect one
host window instead of the active VT. Use the DRM/KMS backend only for
behavior that depends on direct hardware or session ownership.

Complete the [development setup](setup.md) before using this workflow. The
architectural reason for keeping both backends behind one interface is covered
by [ADR-0003](../adr/0003-nested-first-bring-up.md).

## Runtime Model

The nested process is both a Wayland client and a Wayland compositor:

```text
outer Wayland compositor
└── ass xdg-toplevel window (outer Wayland client)
    ├── Vulkan swapchain and ass chrome
    ├── ass Wayland server socket (wayland-N)
    └── inner Wayland clients
```

At startup, ass connects to the inherited `$WAYLAND_DISPLAY`, creates an
`xdg_toplevel`, and presents Flux frames through a Wayland Vulkan surface. It
also creates a separate, auto-named Wayland server socket for applications
managed inside ass.

The host compositor supplies window size, scale, pointer, keyboard, gestures,
and text input. The nested backend converts those events into the same backend
interface used by DRM/KMS. The Wayland server, renderer, window manager, and
shell therefore run through their production paths; only presentation and raw
input ownership differ.

The nested window represents one output named `nested`. Resize the outer
window to exercise logical resize and swapchain recreation. Move it between
outputs to exercise integer or fractional scale changes supported by the host.

## Start the Compositor

Run the following commands from the repository root in a terminal belonging
to an existing Wayland session:

```bash
cargo run -p ass -- --backend nested
```

Pass `--backend nested` explicitly in development commands. The default
`--backend auto` also selects nested mode when `$WAYLAND_DISPLAY` is set, but
the explicit flag prevents an unset environment variable from accidentally
selecting DRM.

A successful start opens an `ass` window and logs both roles:

```text
flux: device created for nested backend; ...
server: listening on WAYLAND_DISPLAY=wayland-N
```

Close the outer window or press `Ctrl+C` in the terminal to stop the nested
session.

## Run Applications Inside ass

Prefer the built-in launcher. It passes the inner socket to the child process
without changing the compositor process's outer `$WAYLAND_DISPLAY`.

To start a test client from another terminal, copy the inner socket name from
the `server: listening` log line and override it for that command. For example:

```bash
WAYLAND_DISPLAY=wayland-1 foot
```

Replace `wayland-1` with the logged value. Keep the existing
`$XDG_RUNTIME_DIR`; the inner socket is created there. A client started without
this override connects to the outer desktop instead.

Run the in-tree IPC client against the nested session when no other ass
process owns `$XDG_RUNTIME_DIR/ass.sock`:

```bash
cargo run -p ass-ctl -- windows
cargo run -p ass-ctl -- outputs
```

Only one ass IPC server can own that path. If the outer compositor is also
ass, the nested process continues without IPC and logs an address-in-use
warning. Isolate the nested runtime when testing two ass processes:

```bash
ass_outer_display=$WAYLAND_DISPLAY
if [[ $ass_outer_display = /* ]]; then
    ass_outer_socket=$ass_outer_display
else
    ass_outer_socket=$XDG_RUNTIME_DIR/$ass_outer_display
fi
ass_nested_runtime=$(mktemp -d)
printf 'nested runtime: %s\n' "$ass_nested_runtime"
XDG_RUNTIME_DIR=$ass_nested_runtime \
WAYLAND_DISPLAY=$ass_outer_socket \
cargo run -p ass -- --backend nested
```

The absolute outer display path lets ass reach the host while the temporary
`$XDG_RUNTIME_DIR` keeps the inner Wayland and IPC sockets separate. In a
second terminal, set `XDG_RUNTIME_DIR` to the printed directory before running
`ass-ctl`. After a graceful compositor exit, remove the empty directory with
`rmdir`.

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
| Check the crate being edited | `cargo check -p ass-shell` |
| Check executable integration | `cargo check -p ass` |
| Run the nested compositor | `cargo run -p ass -- --backend nested` |
| Test one crate | `cargo test -p ass-shell` |
| Validate the workspace | `cargo test --workspace` |
| Build a production artifact | `cargo build --release -p ass` |
| Profile a release build | `cargo build --release -p ass --timings` |

Replace `ass-shell` with the package being edited. Run the release commands
only before delivery, when validating production performance, or when a bug
depends on release optimization. The first build after changing the compiler,
profile, or features is a cold build and can still take several minutes;
subsequent builds reuse `target/debug` artifacts.

`--timings` writes the build report to
`target/cargo-timings/cargo-timing.html`. Use it before changing linker flags,
features, or crate boundaries so optimization work targets the measured
critical path.

### Configuration Changes

Edit `$XDG_CONFIG_HOME/ass/config.toml`, defaulting to
`~/.config/ass/config.toml`, while the nested session runs. The compositor
checks the file each frame and applies a valid change without restarting.
Invalid configuration leaves the previous configuration active and writes a
diagnostic to the log.

Wallpaper environment variables are read at process startup. Restart the
process after changing them. See the
[Configuration Reference](../reference/config.md) for the complete reload
contract.

### Rust Code Changes

Rust code is compiled into the executable loaded by the running process and is
not hot-reloaded. Use Cargo's incremental build, then restart the nested
process:

1. Save the change.
2. Run `cargo check -p ass` or wait for the editor's Rust check to pass.
3. Stop the nested process with `Ctrl+C`.
4. Run `cargo run -p ass -- --backend nested` again.
5. Relaunch any inner client needed for the test.

This manual loop keeps the last working compositor open while a new change
still has compiler errors.

For short visual iterations where losing the current nested state is
acceptable, use a maintained file watcher such as
[Watchexec](https://watchexec.github.io/):

```bash
cargo install --locked watchexec-cli
watchexec --restart --exts rs,toml -- cargo run -p ass -- --backend nested
```

The watcher rebuilds and restarts the process. It does not provide in-process
code hot reload: windows, Wayland connections, IPC state, and other memory
state are recreated after every restart. A compilation error also leaves no
running nested window until the next successful build.

The watcher covers files below the ass repository. After changing Flux, Lens,
or their bindings in the sibling `../optics` tree, rebuild that tree and
restart ass:

```bash
meson compile -C ../optics/build
```

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
before starting ass. The nested backend needs the outer display first.

### A Test Client Opens on the Outer Desktop

Start it with the inner socket logged by ass, or start it through the built-in
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

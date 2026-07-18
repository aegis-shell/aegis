# ass-launch

`ass-launch` starts ordinary detached desktop applications and
compositor-owned, sandboxed Realm applications with the appropriate display
environment and lifecycle.

## Responsibilities

- Expand a desktop entry's `Exec` field through `ass-apps`.
- Preserve the Wayland and XDG environment needed by child applications.
- Honor `Terminal=true` with a configurable terminal emulator.
- Start the process in a new session through `setsid --fork`.
- Redirect detached standard streams away from the compositor.
- Launch Realm applications through fail-closed bubblewrap namespaces with
  explicit filesystem and network grants.
- Return a managed sandbox handle that can pause, resume, terminate, and reap
  the complete Realm cgroup.

## Boundaries

This crate does not discover applications, render a launcher, track resulting
windows, or provide arbitrary shell automation. Discovery belongs to
`ass-apps`; interaction and window lifecycle belong to `ass-shell` and
`ass-server`.

## Runtime Effect

An ordinary successful `launch` returns the spawned process identifier
immediately. Its child runs in a separate session and can survive compositor
shutdown. The host must provide `/usr/bin/setsid`.
`LaunchOpts::foreground` is the diagnostic and test mode: it waits for the
child and returns an error for a nonzero exit.

`launch_managed` requires `LaunchOpts::sandbox` and `/usr/bin/bwrap`. Managed
Realm processes are compositor-owned: dropping the handle kills and reaps the
sandbox. Bubblewrap receives a mount-scoped Realm Wayland listener, read-only
system files, ephemeral home/tmp, and GPU render nodes by default. Its host
socket path is unlinked before application execution is released, while the
private mount continues to support multiple Wayland connections. Network,
KMS device nodes, the session bus, and host user files are absent.

Managed launch also requires cgroup v2 with delegated `cpu`, `memory`, and
`pids` controllers. `prepare_realm_host` creates the compositor host leaf;
successful launches always install hard memory and process limits plus CPU
weight. Missing isolation or delegation rejects the launch.

## Use

```rust
let applications = ass_apps::enumerate();

if let Some(application) = applications.first() {
    let report = ass_launch::launch(application, &Default::default())?;
    println!("spawned {}", report.pid);
}
```

Implement `LaunchSource` to launch a compatible model without performing a
desktop-entry scan.

Use `launch_managed` for Realm lifecycle ownership:

```rust
let mut process = ass_launch::launch_managed(application, &realm_options)?;
process.pause()?;
process.resume()?;
process.terminate()?;
```

## Related Documentation

- [Application launcher decision](../../docs/adr/0022-application-launcher.md)
- [How to Use AI Workspaces](../../docs/how-to/ai-workspaces.md)

# aegis-launcher

`aegis-launcher` starts ordinary detached desktop applications and
compositor-owned, sandboxed Interaction Domain applications with the appropriate display
environment and lifecycle.

## Responsibilities

- Expand a desktop entry's `Exec` field through `aegis-desktop-entries`.
- Preserve the Wayland and XDG environment needed by child applications.
- Honor `Terminal=true` with a configurable terminal emulator.
- Start the process in a new session through `setsid --fork`.
- Redirect detached standard streams away from the compositor.
- Launch Interaction Domain applications through fail-closed bubblewrap namespaces with
  no host network or user-file mounts.
- Return a managed sandbox handle that can pause, resume, terminate, and reap
  the complete Interaction Domain cgroup.

## Boundaries

This crate does not discover applications, render a launcher, track resulting
windows, or provide arbitrary shell automation. Discovery belongs to
`aegis-desktop-entries`; interaction and window lifecycle belong to `aegis-shell` and
`aegis-compositor`.

## Runtime Effect

An ordinary successful `launch` returns the spawned process identifier
immediately. Its child runs in a separate session and can survive compositor
shutdown. The host must provide `/usr/bin/setsid`.
`LaunchOpts::foreground` is the diagnostic and test mode: it waits for the
child and returns an error for a nonzero exit.

`launch_managed` requires `LaunchOpts::sandbox` and `/usr/bin/bwrap`. Managed
Interaction Domain processes are compositor-owned: dropping the handle kills and reaps the
sandbox. Bubblewrap receives a mount-scoped Interaction Domain Wayland listener, read-only
system files, ephemeral home/tmp, and GPU render nodes by default. Its host
socket path is unlinked before application execution is released, while the
private mount continues to support multiple Wayland connections. Network,
KMS device nodes, the session bus, and host user files are absent.

Managed launch also requires cgroup v2 with delegated `cpu`, `memory`, and
`pids` controllers. `prepare_interaction_domain_host` creates the compositor host leaf;
successful launches always install hard memory and process limits plus CPU
weight. Missing isolation or delegation rejects the launch.

## Use

```rust
let applications = aegis_apps::enumerate();

if let Some(application) = applications.first() {
    let report = aegis_launch::launch(application, &Default::default())?;
    println!("spawned {}", report.pid);
}
```

Implement `LaunchSource` to launch a compatible model without performing a
desktop-entry scan.

Use `launch_managed` for Interaction Domain lifecycle ownership:

```rust
let mut process = aegis_launch::launch_managed(application, &interaction_domain_options)?;
process.pause()?;
process.resume()?;
process.terminate()?;
```

## Related Documentation

- [Application launcher decision](../../docs/adr/0022-application-launcher.md)
- [How to Use Agent Workspaces](../../docs/how-to/ai-workspaces.md)

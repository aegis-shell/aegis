# ass-launch

`ass-launch` starts desktop applications with the detachment and environment
required for them to outlive and reconnect to the compositor.

## Responsibilities

- Expand a desktop entry's `Exec` field through `ass-apps`.
- Preserve the Wayland and XDG environment needed by child applications.
- Honor `Terminal=true` with a configurable terminal emulator.
- Start the process in a new session through `setsid --fork`.
- Redirect detached standard streams away from the compositor.

## Boundaries

This crate does not discover applications, render a launcher, track resulting
windows, or provide arbitrary shell automation. Discovery belongs to
`ass-apps`; interaction and window lifecycle belong to `ass-shell` and
`ass-server`.

## Runtime Effect

A successful launch returns the spawned process identifier immediately. The
child runs in a separate session and can survive compositor shutdown. The host
must provide `/usr/bin/setsid`.

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

## Related Documentation

- [Application launcher decision](../../docs/adr/0022-application-launcher.md)
- [Workspace layout](../../docs/dev/project-layout.md)


# aegis-lock

`aegis-lock` is the first-party secure session locker for Aegis. It is a
standalone Wayland client of `ext-session-lock-v1`; the compositor remains the
authority that hides normal content, routes input exclusively to lock surfaces,
and retains a fail-closed frame if the client exits while locked.

The locker owns presentation and authentication only: one responsive surface
per output, the Aegis design language, keyboard input, PAM authentication,
credential-memory clearing, and readiness notification for sleep/idle
coordination.

## Development Preview

The feature-gated `aegis-lock-preview` target renders the same lock content in
an ordinary Wayland window without acquiring a session lock or calling PAM.
It is not a distribution artifact. Start an interactive fake-password session
with:

```bash
cargo run --locked -p aegis-lock --features dev-preview \
  --bin aegis-lock-preview -- --password 0000
```

Use only a development fake value: command arguments are not a safe place for
an account password. See the contributor
[Lock-Screen Testing](../../docs/dev/lock-screen-testing.md) workflow for
visual states, agent captures, nested integration, and physical validation.

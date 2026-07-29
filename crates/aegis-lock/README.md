# aegis-lock

`aegis-lock` is the first-party secure session locker for Aegis. It is a
standalone Wayland client of `ext-session-lock-v1`; the compositor remains the
authority that hides normal content, routes input exclusively to lock surfaces,
and retains a fail-closed frame if the client exits while locked.

The locker owns presentation and authentication only: one responsive surface
per output, the Aegis design language, keyboard input, PAM authentication,
credential-memory clearing, and readiness notification for sleep/idle
coordination.

# ADR-0007: Logging facade and the `Backend` input contract

- Status: Accepted
- Date: 2026-06-18

## Context

Two cross-cutting shapes were missing when milestone M1 design began:

1. **Observability.** Every crate reached for `eprintln!` ad-hoc. There was no
   level discipline (info vs warn vs error), no way for a future subscriber to
   tap the stream, and no consistent way to silence per-frame noise. The
   AI-adaptation phase ([Architecture](../explanation/architecture.md) M5+)
   wants compositor events as an introspection channel; bare stderr is not
   that.

2. **Input plumbing.** The `Backend` trait declared `size` and `dispatch` but
   no input accessor. The nested backend never bound the host seat, the server
   advertised zero capabilities, and the shell's `Input` came from
   `Input::default()` constructed once outside the main loop. There was no
   place *to* wire input even if every component were implemented.

The two changes are coupled: an input contract without a logging seam leaves
bring-up blind, and a logging seam without new traffic to push through it
demonstrates nothing.

## Decision

1. **Adopt the `log` facade in every workspace crate, with `env_logger` as
   the single concrete impl in the binary.** Libraries depend on `log` only;
   the binary's `main` calls `env_logger::Builder::from_env(...).try_init()`
   before any crate logs. `RUST_LOG` controls verbosity; the default level is
   `info` so the bring-up sequence is visible without configuration.

2. **The `Backend` trait grows two methods, both required (no default
   impls).** `take_input(&mut self) -> Vec<InputEvent>` drains buffered input
   since the last call; `take_resize(&mut self) -> Option<Size>` reports a
   pending host reconfigure. Implementors cannot accidentally fall through to
   a silent no-op. The nested backend currently returns an empty input vector
   (M1 wires the host seat); its `take_resize` is the existing logic moved
   onto the trait.

3. **Input types live in `aegis-core::input`, not in `aegis-backend`.** Backends
   emit `InputEvent`; the server and shell consume it. Putting the types in
   `aegis-core` means the consumers never need a backend dep. Coordinates are
   compositor logical space; button and key codes are Linux input-event
   codes so the server can hand them to `wl_pointer.button` and
   `wl_keyboard.key` without translation.

## Alternatives

- **`tracing` instead of `log`.** Rejected for now: `tracing` is heavier and
   adds span discipline the codebase does not yet exercise. The `log` API is
   a strict subset conceptually; migrating later is mechanical if structured
   spans become worth it.
- **A backend-owned `Input` type (in `aegis-backend`).** Rejected: it forces
   every consumer of input (the server, the shell, the eventual
   introspection API) to depend on the backend crate, inverting the
   dependency direction the architecture wants.
- **Default impls on the new `Backend` methods.** Rejected: a silent no-op
   is exactly the bug the trait change is meant to prevent. A backend that
   forgets input should fail to compile.

## Consequences

- Every library crate gains a `log` dependency; the binary gains
  `env_logger`. Both are tiny and ubiquitous.
- The compositor's startup is observable with `RUST_LOG=debug cargo run`,
   and quiet by default. The previous "first frame presented" line, the
   dma-buf import diagnostics, and the "server listening on" line all flow
   through one filter.
- M1 work (host seat binding, server-side pointer/keyboard, shell input
   snapshot) plugs into a seam that already exists. No further trait or
   type changes are required to land input end-to-end.
- The eventual AI-adaptation introspection API can subscribe to the same
   `log` stream as a low-budget semantic channel before a structured one is
   designed.

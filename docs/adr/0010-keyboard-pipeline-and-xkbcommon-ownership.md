# ADR-0010: Keyboard pipeline and xkbcommon ownership

- Status: Superseded by [ADR-0040](0040-realms-seats-and-transferable-interaction-authority.md)
- Date: 2026-06-18

## Context

With the pointer pipeline landed ([ADR-0009](0009-input-pipeline-and-pointer-focus.md)),
M1 input is half-done. A real client cannot type into a surface under ass:
the seat advertises no keyboard capability, no keymap is sent on bind, and
`InputEvent::Key` is dropped by `Server::forward_input`. The minimum needed
for "input routed to the focused client" is keyboard end-to-end.

A Wayland keyboard requires three things the pointer did not:

1. A **keymap**. The `wl_keyboard.keymap` event arrives before any key event
   and carries a file descriptor whose contents are an xkbcommon text-format
   keymap. Without it, a client cannot decode keycodes into keysyms.
2. **Modifier state**. `wl_keyboard.modifiers` carries the depressed/latched/
   locked/effective-group masks. The compositor must track the xkbcommon
   state and emit modifier updates when keys change them.
3. **Keyboard focus** that is decoupled from pointer focus (click-to-focus
   sets keyboard focus only on press; motion does not change it).

## Decision

1. **The server owns the xkbcommon keymap and state.** A new
   `ass_server::keyboard::Keyboard` struct compiles the default
   `"evdev"/"pc104"/"us"` RMLVO at startup (matching Weston, wlroots, and
   Mutter defaults), serializes it to a sealed memfd, and owns the
   `xkb_state`. The state lives for the server's lifetime so the keymap fd
   stays valid for any later client bind.

2. **The keymap file is a sealed memfd.** `memfd_create` with
   `MFD_CLOEXEC | MFD_ALLOW_SEALING`, `ftruncate` to the exact keymap size,
   `mmap`+`memcpy` to write the bytes, `munmap`, then
   `F_ADD_SEALS` with `SEAL_SEAL | SEAL_SHRINK | SEAL_GROW | SEAL_WRITE`. A
   hostile client cannot mutate the shared file to corrupt another client's
   keymap. Each client bind dups the fd; libwayland closes the dup after
   queueing the event, the original stays open.

3. **The seat advertises keyboard capability conditionally** — only when the
   xkbcommon keymap compiled successfully at startup. Failure is logged and
   the seat falls back to pointer-only, so a broken xkbcommon install does
   not crash the compositor.

4. **Click-to-focus for keyboard.** `pointer_button` on a press edge calls
   `change_keyboard_focus` with the surface under the pointer. The
   transition posts `wl_keyboard.leave` (with serial) to the old focus's
   client's keyboard resources and `wl_keyboard.enter` (with serial, an
   empty keys array, and the new surface) to the new focus's. Keyboard focus
   and pointer focus are tracked independently: pointer motion changes
   pointer focus only.

5. **Key events flow backend → server → client.** The nested backend binds
   the host `wl_keyboard`, translates `wl_keyboard.key` to
   `InputEvent::Key` (evdev scancode, press/release), and buffers it. The
   server's `keyboard_key` advances the xkbcommon state via
   `State::update_key`, reads the new modifier mask via `serialize_mods` /
   `serialize_layout`, and posts `wl_keyboard.modifiers` (always) followed
   by `wl_keyboard.key` to the focused client's keyboard resources.

6. **The host's keymap and modifier events are consumed, not forwarded.**
   The host sends its own `wl_keyboard.keymap` (which we close to avoid an
   fd leak) and its own `wl_keyboard.modifiers` (which we ignore, since we
   compute ours from `xkb_state` update-by-update). The host's keymap and
   ours match in practice because both use the default RMLVO, but the design
   does not depend on it.

## Alternatives

- **Forward the host's keymap verbatim.** Rejected: a keymap fd is per-bind
   per-client, and the host may seal or close its copy on terms we don't
   control. Owning our own keymap gives us a stable fd we can dup freely.
- **Forward the host's modifier events.** Rejected: we'd be double-counting
   (our `xkb_state` and the host's), and the host's masks reflect the host
   client's view, not the compositor's authoritative state.
- **Skip the memfd and use a plain tmpfile.** Rejected: sealing is the only
   thing that prevents a hostile client from `mmap`+write into the shared
   file to attack other clients. The cost is one `fcntl` call.
- **Suppress redundant `wl_keyboard.modifiers` posts.** Rejected for now:
   always-post is simpler, the client-side xkbcommon treats no-op updates
   cheaply, and a delta check is a trivial future optimization.
- **Track held keys for resend on enter.** Deferred: M1 sends an empty keys
   array. Real compositors resend the held set so a refocused client keeps
   modifier state consistent; that is a polish item once the keyboard
   pipeline stabilizes.

## Consequences

- M1 input is end-to-end: a client under ass receives pointer and keyboard
  events with a real keymap and modifier tracking. The Quit button works;
  focused clients type.
- The server gains a hard dependency on `xkbcommon` (the safe Rust wrapper,
  which itself links `libxkbcommon`). `docs/dev/setup.md` should mention
  the system package.
- The memfd keymap is allocated once per server lifetime and never resized.
  The default xkb keymap is around 30–60 KB; one fd per process.
- Keyboard repeat defaults are baked in (rate 25, delay 250 ms) — Weston
  and Mutter's defaults. A per-output or per-seat policy is a future
  generalization.
- Touch is the only remaining input modality not on the pipeline; its
  absence does not block M1.

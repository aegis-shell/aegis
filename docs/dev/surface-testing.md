# Surface Testing

How the interactive shell surfaces are tested: the test model, the
code-organization rules every layer follows, and the commands for each
surface — the command panel (compositor chrome, ADR-0080) and the lock screen
(the `tessera-lock` production surface). A result from one layer does not stand
in for a result from another.

## Test Model

| Layer | Environment | Covers | Does not cover |
|-------|-------------|--------|----------------|
| Unit and logic | In-crate tests, no GPU or live sources | Layout bounds, reduced-motion snapping, backdrop-effect constraints, formatting helpers, persona resolution | Rendering, live sources, interaction |
| Preview | Development-only harness binary, ordinary Wayland window | Full rendering and interaction of one surface, isolated from the session | Session integration, exclusive input, PAM, physical outputs |
| Nested integration | Production binaries inside a nested Tessera compositor | Real chrome and lock surfaces, input routing, protocols, fail-closed behavior | Physical output power, VT ownership, suspend |
| Physical | Packaged direct DRM/KMS session | Installed PAM policy, every physical output, capture refusal, display-off, suspend and resume | Safe unattended iteration |

Start with the cheapest layer that can fail for the change: unit tests for
logic, preview for rendering and interaction of one surface, nested
integration for protocol, input, and multi-surface behavior. Finish on a
physical session only when the change crosses a hardware or authentication
boundary.

## Code Conventions

Every testing facility is arranged in code according to its layer. New test
and debug facilities follow these rules.

### Unit Layer: Pure Logic in Crate Tests

Presentation logic is written as pure data transformations and tested in
in-crate `#[cfg(test)]` modules with `cargo nextest run --locked -p <crate>
--lib`. Provide a constructor that removes external sources — for example
`CommandPanel::without_sources()` — so tests need no GPU, bus, or tray. Unit
tests never open windows or devices.

### Presentation Toggles: Debug-Gated Environment Variables

A production binary may expose debug-gated environment variables that change
**presentation only** — which surface is open at startup, what demo content is
seeded. `TESSERA_COMMAND_PANEL_OPEN` is the example: it opens the panel at
startup in debug builds and seeds demo notifications. Environment variables
must never alter authentication, authorization, or capture behavior. See
[Development Environment Variables](environment-variables.md) for the complete
reference.

### Inspection Harnesses: Separate Development-Only Binaries

When a surface needs parameterized inspection — held states, compositions,
sizes — build it as a separate binary target in the package, gated by a
non-default feature (`tessera-lock-preview`, `required-features =
["dev-preview"]`). The production binary compiles no harness code and carries
no `cfg` branches for it: both binaries are thin hosts over the shared
library, which is what makes the preview render exactly what production
renders.

Security-sensitive surfaces take this rule absolutely. The lock screen is
fail-closed: a preview mode inside the production locker — even debug-gated —
would put a simulated-credential path into a security binary. Simulated
authentication lives only in the harness binary.

[Verify Production Exclusion](#verify-production-exclusion) enforces the
boundary at packaging time with target-availability assertions.

### One Command Style

Every command runs through cargo:

```bash
cargo run --locked -p <package> [--bin <bin>] -- [ARGS]
```

Build once with `cargo build --locked` and use the `target/<profile>/<binary>`
path only when a script needs a stable path across many invocations.

### One Isolation Block

Every nested start of `tessera` uses the same session-scoped isolation block:

```bash
mkdir -p /tmp/tessera-dev/tessera
TESSERA_BACKEND=nested \
XDG_DATA_HOME=/tmp/tessera-dev \
XDG_DATA_DIRS=$HOME/.local/share:/usr/local/share:/usr/share \
cargo run --locked -p tessera
```

The fresh data directory keeps the nested instance off the live session's
exclusive audit journal and application caches. `mkdir -p` is idempotent; the
recipes below repeat it so each block stands alone. Only one Tessera process can
own `tessera.sock`: a nested instance started beside a live session logs an
address-in-use warning and runs without IPC.

## Test the Command Panel

### Unit Tests

Panel presentation logic follows the unit-layer rule: pure logic, no GPU or
live sources. Run the suite:

```bash
cargo check --locked -p tessera-command-panel
cargo nextest run --locked -p tessera-command-panel --lib
```

The suite covers cluster bounds on small displays, reduced-motion reveal
snapping, the constraint that the panel never requests backdrop effects,
formatting helpers, and persona profile resolution. It cannot catch rendering
faults such as a frame arena overflow; escalate to the preview layer when the
change touches rendering.

### Panel Preview in a Nested Session

The panel is compositor chrome, so its preview runs the production compositor
with a presentation toggle rather than a harness binary.

The host compositor intercepts global `Super` shortcuts in nested sessions, so
open the panel one of two ways.

Auto-open on startup (zero keystrokes) — the shared isolation block plus one
toggle, debug builds only:

```bash
mkdir -p /tmp/tessera-dev/tessera
TESSERA_COMMAND_PANEL_OPEN=1 \
TESSERA_BACKEND=nested \
XDG_DATA_HOME=/tmp/tessera-dev \
XDG_DATA_DIRS=$HOME/.local/share:/usr/local/share:/usr/share \
cargo run --locked -p tessera
```

Debug builds seed a few demo notifications so the stream has content without a
bus. Close and reopen the panel with `Super+S` when the host does not claim
it, `Escape`, or a click on the scrim.

Non-Super keybinding — key combinations such as `Ctrl+Alt+S` reach the nested
window directly. Add a development keybinding in `config.toml`:

```toml
[[keybind]]
mods = ["ctrl", "alt"]
key = "s"
action = "command_panel"
```

Nested limits: the instance cannot modeset physical outputs or configure
libinput devices, so validate display and touchpad settings under DRM/KMS.
Follow [Nested Backend Development](nested-backend.md) for socket discovery,
process replacement, and backend limits.

### Capture Panel Pixels for an Agent

Keep the physical session unlocked. Start the nested compositor with the panel
auto-opened, then drive the outer session's CLI from another terminal:

```bash
windows=$(cargo run --locked -p tessera -- window -j)

nested_id=$(printf '%s' "$windows" | jq -r \
  'map(select(.app_id == "tessera" and .title == "tessera")) | last | .id')

nested_region=$(printf '%s' "$windows" | jq -r \
  'map(select(.app_id == "tessera" and .title == "tessera")) | last |
    "\(.position.x),\(.position.y),\(.size.w),\(.size.h)"')

cargo run --locked -p tessera -- window focus "$nested_id"
cargo run --locked -p tessera -- display capture \
  --region "$nested_region" \
  /tmp/tessera-nested-panel.png
```

Focus before capture so the terminal does not raise itself above the target.
Capture queues asynchronous PNG encoding: treat the image as ready only after
the destination exists and has a nonzero size. On HiDPI outputs the PNG
dimensions are the physical-pixel equivalent of the logical region.

## Test the Lock Screen

### Run the Development Preview

The lock preview follows the inspection-harness rule: a development-only
binary target in the `tessera-lock` package, selected by the non-default
`dev-preview` feature:

```bash
cargo run --locked -p tessera-lock \
  --features dev-preview \
  --bin tessera-lock-preview -- [OPTIONS]
```

Use `cargo build --locked` with the same package, feature, and target when a
script needs the stable path `target/debug/tessera-lock-preview`.

The Preview creates an ordinary `xdg_toplevel` with app ID
`dev.tessera.LockPreview`. It uses the production `LockState`, identity loader,
avatar resources, and renderer, but it never creates an `ext_session_lock_v1`
object or calls PAM. Enter any nonempty text and press `Enter` to simulate an
accepted result; press `Escape` to close without submitting. `Backspace` and
`Ctrl+U` exercise the production credential-editing behavior.

Preview options:

| Option | Values | Purpose |
|--------|--------|---------|
| `--composition` | `cinematic`, `centered`, `bsod` | Selects the complete UI composition. `--style` is a compatibility alias. |
| `--background` | Image path | Overrides the background for this process only; `bsod` ignores it. An invalid image falls back to the bundled artwork. |
| `--state` | `ready`, `typing`, `checking`, `rejected`, `unavailable`, `ambient` | Holds a state for inspection. |
| `--result` | `accepted`, `rejected`, `unavailable` | Result of a later interactive submission; use for a deterministic one-state inspection. |
| `--password` | Fake value | Only this exact value closes the Preview; any other nonempty value produces the production rejection state. Use for an interactive accept/reject loop. |
| `--size` | `WIDTHxHEIGHT` | Fixed logical window size for small, standard, ultrawide, and portrait layout checks. |
| `--ready-fd` | Descriptor | Writes one byte after the first presented frame, for automation that must not parse logs. |

`--password` and `--result` are mutually exclusive. Never pass a real account
password: command-line arguments may be retained in shell history and visible
in the process list. The fake value never reaches PAM or Tessera configuration.

The Preview reads `[lock_screen]` from the normal Tessera configuration; the
options above scope a single process instead of editing that file. For
translated lock copy, set the process locale:

```bash
LC_ALL=zh_CN.UTF-8 \
cargo run --locked -p tessera-lock \
  --features dev-preview \
  --bin tessera-lock-preview -- --state unavailable
```

Interactive submissions retain the checking state briefly so the selected
composition's validation feedback can be inspected. From submission until the
result arrives, the shared lock state freezes typing, deletion, clearing, and
repeated submission while retaining the entered marks as locked feedback. If
`Enter` is pressed during rejection backoff, the attempt remains pending and
starts automatically at the retry deadline. Cinematic's cool light sweep runs
at a fixed rate from the credential rail's left edge to its right, independent
of password length and backoff; the production lock keeps the same feedback
active until PAM replies.

What to capture per composition — capture both compositions after changing
shared typography, identity, credential, or status rendering:

- `cinematic`: an empty-password frame (neutral rail, no marks), a typing
  frame (marks only, never empty slots implying a length), and a rejected
  frame (red rail, no text error). Use a short recording for the rejection
  shake.
- `bsod`: engaged, typing, checking, and rejected states at compact
  (`390x844`), desktop (`1280x800`), and ultra (`3440x1440`) size. The
  composition has no input box: the sad face, wrapped headline, keystroke
  counter, and support block must stay inside the side margins, and the QR
  module must keep its quiet zone clear of the support lines. The counter
  narrates the authentication state — the entered-character count while
  typing, a static verifying line while checking, and zero after rejection —
  and a rejected attempt also updates the stop code in the support block.

Production `tessera-lock` uses the independent `[lock_screen.background]`
configuration described in the
[Configuration Reference](../reference/config.md#lock-screen).

### Capture Lock Preview Pixels for an Agent

Keep the physical session unlocked. Start the Preview in one terminal, then
drive the outer session's CLI from another:

```bash
windows=$(cargo run --locked -p tessera -- window -j)

preview_id=$(printf '%s' "$windows" | jq -r \
  'map(select(.app_id == "dev.tessera.LockPreview")) | last | .id')

preview_region=$(printf '%s' "$windows" | jq -r \
  'map(select(.app_id == "dev.tessera.LockPreview")) | last |
    "\(.position.x),\(.position.y),\(.size.w),\(.size.h)"')

cargo run --locked -p tessera -- window focus "$preview_id"
cargo run --locked -p tessera -- display capture \
  --region "$preview_region" \
  /tmp/tessera-lock-preview.png
```

The capture readiness and HiDPI notes from the panel recipe apply unchanged.

### Nested Integration: the Production Lock

Use Preview results only for presentation. Validate session-lock behavior with
the unmodified production target inside nested Tessera, using the shared
isolation block:

```bash
mkdir -p /tmp/tessera-dev/tessera
TESSERA_BACKEND=nested \
XDG_DATA_HOME=/tmp/tessera-dev \
XDG_DATA_DIRS=$HOME/.local/share:/usr/local/share:/usr/share \
cargo run --locked -p tessera
```

Copy the inner socket name from the startup log:

```text
server: listening on WAYLAND_DISPLAY=wayland-N
```

Start the production locker on that socket:

```bash
WAYLAND_DISPLAY=wayland-N \
cargo run --locked -p tessera-lock --bin tessera-lock --no-default-features
```

The inner compositor is now genuinely locked. The unlocked outer compositor
can capture the nested Tessera window for review without weakening the inner
capture policy.

Enter the account password to test the complete PAM transition. For a visual
or protocol-only test, stop the entire nested compositor to recover. Do not
kill only `tessera-lock` after secure confirmation: the inner compositor must
retain its fail-closed frame. Follow
[Nested Backend Development](nested-backend.md) for socket discovery, process
isolation, and nested-backend limits.

## Test a Physical Session

Perform physical testing only after preview and nested checks pass. Keep
another TTY available, and perform these checks in order:

1. Lock and unlock from the active desktop.
2. Confirm capture refusal on the locked surface and display-off.
3. Suspend and resume while locked; confirm the fail-closed frame survives
   and input is required again.
4. Repeat on every physical output.

Validate the exact PAM service with `pamtester` first. For the command panel,
physical sessions validate the display and touchpad settings that nested mode
can only observe. For the full hardware procedure, see
[VT/DRM Manual Testing](vt-drm-testing.md).

## Select Tests by Change

| Change | Required checks |
|--------|-----------------|
| Panel layout, color, typography or localized copy | Unit tests plus panel preview at representative sizes, scales, locales, and visual states |
| Panel quick controls, tabs, tray, or menu navigation | Panel preview: open and close, switch tabs, descend and back in menus, tray activation |
| Panel notification stream or network monitor | Panel preview: seeded demo notifications and live source rendering |
| Lock layout, color, typography or localized copy | Preview both compositions at representative sizes, scales, locales and visual states |
| Independent lock background or artwork scrim | Preview with `--background`, then start a fresh production lock using `[lock_screen.background]` |
| Stop-screen (`bsod`) composition | Preview engaged, typing, checking, and rejected states at compact, desktop, and ultra sizes; confirm the keystroke counter and stop code track each state |
| Avatar image, VRM or animation | Preview initial frame, motion, hot reload and fallback avatar |
| Credential state machine or authentication feedback | `tessera-lock` unit tests plus Preview submission results |
| Lock-surface lifecycle or input routing | Nested lock, including client exit and outer capture |
| PAM service or account policy | `pamtester` followed by physical lock and unlock |
| Capture or ScreenCast policy | Nested policy check followed by physical-session verification |
| Idle, display-off, suspend or resume | Physical DRM/KMS session |

## Verify Production Exclusion

The `dev-preview` feature changes target availability, not production lock
logic. Build the production target explicitly in a fresh target directory and
assert that only it exists:

```bash
tessera_prod_target=$(mktemp -d)
cargo build --locked --release \
  --target-dir "$tessera_prod_target" \
  -p tessera-lock --bin tessera-lock --no-default-features

test -x "$tessera_prod_target/release/tessera-lock"
test ! -e "$tessera_prod_target/release/tessera-lock-preview"
```

Distribution builds must not enable `dev-preview`, and install manifests must
name `target/release/tessera-lock` explicitly. Do not install binaries through a
`target/release/tessera-*` wildcard.

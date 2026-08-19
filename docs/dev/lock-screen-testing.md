# Lock-Screen Testing

Use separate test layers for lock-screen presentation, session-lock
integration, and physical security behavior. A result from one layer does not
stand in for a result from another.

## Test Layers

| Layer | Environment | Covers | Does not cover |
|-------|-------------|--------|----------------|
| Preview | Ordinary Wayland window | Layout, localization, avatar rendering, authentication states, keyboard and pointer presentation | Session-lock protocol, exclusive input, PAM, capture refusal |
| Nested lock | `aegis-lock` inside a nested Aegis compositor | Production lock surfaces, secure-frame handshake, input routing, fail-closed behavior | Physical output power, VT ownership, suspend |
| Physical lock | Packaged direct DRM/KMS session | Installed PAM policy, every physical output, capture refusal, display-off, suspend and resume | Safe unattended iteration |

Start with Preview for visual changes, move to a nested lock for protocol and
input changes, and finish on a physical session only when the change crosses a
hardware or authentication boundary.

## Run the Development Preview

`aegis-lock-preview` is a development-only binary target in the `aegis-lock`
package. The `dev-preview` feature is not enabled by default and is required
to select that target. Run it with one command:

```bash
cargo run --locked -p aegis-lock \
  --features dev-preview \
  --bin aegis-lock-preview -- [OPTIONS]
```

Every later example invokes `aegis-lock-preview` this way; only `[OPTIONS]`
changes. Use `cargo build` with the same package, feature, and target when a
script needs the stable path `target/debug/aegis-lock-preview`.

The Preview creates an ordinary `xdg_toplevel` with app ID
`dev.aegis.LockPreview`. It uses the production `LockState`, identity loader,
avatar resources, and renderer, but it never creates an `ext_session_lock_v1`
object or calls PAM. Enter any nonempty text and press `Enter` to simulate an
accepted result; press `Escape` to close without submitting. `Backspace` and
`Ctrl+U` exercise the production credential-editing behavior.

### Preview Options

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
in the process list. The fake value never reaches PAM or Aegis configuration.

The Preview reads `[lock_screen]` from the normal Aegis configuration; the
options above scope a single process instead of editing that file. For
translated lock copy, set the process locale:
`LC_ALL=zh_CN.UTF-8 aegis-lock-preview --state unavailable`.

Interactive submissions retain the checking state briefly so the selected
composition's validation feedback can be inspected. From submission until the
result arrives, the shared lock state freezes typing, deletion, clearing, and
repeated submission while retaining the entered marks as locked feedback. If
`Enter` is pressed during rejection backoff, the attempt remains pending and
starts automatically at the retry deadline. Cinematic's cool light sweep runs
at a fixed rate from the credential rail's left edge to its right,
independent of password length and backoff; the production lock keeps the
same feedback active until PAM replies.

### What to Capture per Composition

Capture both compositions after changing shared typography, identity,
credential, or status rendering.

For `cinematic`, include an empty-password frame, a typing frame, and a
rejected frame. The empty frame must have a neutral rail with no password
marks; the typing frame must show only marks for entered characters, never
empty slots that imply an expected password length; the rejected frame must
show a red rail without a text error. Use a short recording rather than a
still frame when reviewing the rejection shake.

For `bsod`, preview the engaged, typing, checking, and rejected states at a
compact (`390x844`), desktop (`1280x800`), and ultra (`3440x1440`) size. The
composition has no input box: the sad face, wrapped headline, keystroke
counter, and support block must stay inside the side margins, and the QR
module must keep its quiet zone clear of the support lines. The counter
narrates the authentication state in the page's own voice — the
entered-character count while typing, a static verifying line while checking,
and zero after rejection — and a rejected attempt also updates the stop code
in the support block.

Production `aegis-lock` uses the independent `[lock_screen.background]`
configuration described in the
[Configuration Reference](../reference/config.md#lock-screen).

## Capture Preview Pixels for an Agent

Keep the physical Aegis session unlocked. Start the Preview in one terminal,
then locate and capture it through the outer compositor in another:

```bash
preview_id=$(target/debug/aegis window -j | \
  jq -r 'map(select(.app_id == "dev.aegis.LockPreview")) | last | .id')

preview_region=$(target/debug/aegis window -j | \
  jq -r 'map(select(.app_id == "dev.aegis.LockPreview")) | last |
    "\(.position.x),\(.position.y),\(.size.w),\(.size.h)"')

target/debug/aegis window focus "$preview_id"
target/debug/aegis display capture \
  --region "$preview_region" \
  /tmp/aegis-lock-preview.png
```

Focusing before capture keeps the terminal from raising itself above the
target window. The capture command queues asynchronous PNG encoding; treat
the image as ready only after the destination exists and has a nonzero size.
On HiDPI outputs the PNG dimensions are the physical-pixel equivalent of the
logical region.

## Test the Production Lock in a Nested Compositor

Use Preview results only for presentation. Validate session-lock behavior with
the unmodified production target inside nested Aegis. Start the compositor
with the nested backend selected explicitly:

```bash
AEGIS_BACKEND=nested cargo run --locked -p aegis
```

When an Aegis session already runs on the desktop, apply the XDG isolation
from the [nested testing
recipe](environment-variables.md#quick-recipe-nested-testing-inside-a-live-aegis-session)
first — otherwise the nested instance fails on the production audit journal's
exclusive `flock` and cannot own `aegis.sock`.

Copy the inner socket name from the startup log:

```text
server: listening on WAYLAND_DISPLAY=wayland-N
```

Build and start the production locker on that socket:

```bash
cargo build --locked -p aegis-lock --bin aegis-lock --no-default-features
WAYLAND_DISPLAY=wayland-N target/debug/aegis-lock
```

The inner compositor is now genuinely locked. The unlocked outer compositor
can capture the nested Aegis window for review without weakening the inner
capture policy.

Enter the account password to test the complete PAM transition. For a visual
or protocol-only test, stop the entire nested compositor to recover. Do not
kill only `aegis-lock` after secure confirmation: the inner compositor must
retain its fail-closed frame. Follow
[Nested Backend Development](nested-backend.md) for socket discovery, process
isolation, and nested-backend limits.

## Test a Physical Session

Perform physical testing only after Preview and nested checks pass. Validate
the exact PAM service with `pamtester`, keep another TTY available, and
trigger the lock without waiting for the idle deadline with
`aegis-idle --lock-now`.

Follow [How to Install and Verify the Lock Screen](../how-to/lock-screen.md)
for authentication and secure-transition checks. Follow
[VT/DRM Manual Testing](vt-drm-testing.md) for output, VT, suspend, and
hardware recovery checks.

## Select Tests by Change

| Change | Required checks |
|--------|-----------------|
| Layout, color, typography or localized copy | Preview both compositions at representative sizes, scales, locales and visual states |
| Independent lock background or artwork scrim | Preview with `--background`, then start a fresh production lock using `[lock_screen.background]` |
| Stop-screen (`bsod`) composition | Preview engaged, typing, checking, and rejected states at compact, desktop, and ultra sizes; confirm the keystroke counter and stop code track each state |
| Avatar image, VRM or animation | Preview initial frame, motion, hot reload and fallback avatar |
| Credential state machine or authentication feedback | `aegis-lock` unit tests plus Preview submission results |
| Lock-surface lifecycle or input routing | Nested lock, including client exit and outer capture |
| PAM service or account policy | `pamtester` followed by physical lock and unlock |
| Capture or ScreenCast policy | Nested policy check followed by physical-session verification |
| Idle, display-off, suspend or resume | Physical DRM/KMS session |

## Verify Production Exclusion

The Preview feature changes target availability, not production lock logic.
Build the production target explicitly in a fresh target directory and assert
that only it exists:

```bash
aegis_prod_target=$(mktemp -d)
cargo build --locked --release \
  --target-dir "$aegis_prod_target" \
  -p aegis-lock --bin aegis-lock --no-default-features

test -x "$aegis_prod_target/release/aegis-lock"
test ! -e "$aegis_prod_target/release/aegis-lock-preview"
```

Distribution builds must not enable `dev-preview`, and install manifests must
name `target/release/aegis-lock` explicitly. Do not install binaries through a
`target/release/aegis-*` wildcard.

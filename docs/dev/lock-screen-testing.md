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

`aegis-lock-preview` is a development-only binary target in the
`aegis-lock` package. The `dev-preview` feature is not enabled by default and
is required to select that target. For ordinary interactive iteration, build
and run it in one command:

```bash
cargo run --locked \
  -p aegis-lock \
  --features dev-preview \
  --bin aegis-lock-preview
```

Use a separate build when an agent, capture script, or repeated test session
needs a stable executable path without invoking Cargo again:

```bash
cargo build --locked \
  -p aegis-lock \
  --features dev-preview \
  --bin aegis-lock-preview
```

Then run that artifact directly:

```bash
target/debug/aegis-lock-preview
```

The Preview creates an ordinary `xdg_toplevel` with app ID
`dev.aegis.LockPreview`. It uses the production `LockState`, identity loader,
avatar resources, and renderer, but it never creates an
`ext_session_lock_v1` object or calls PAM.

Enter any nonempty text and press `Enter` to simulate an accepted
authentication result and close the Preview. Press `Escape` to close without
submitting. `Backspace` and `Ctrl+U` exercise the production credential-editing
behavior.

To exercise both the accepted and rejected paths interactively, select a
development-only fake password:

```bash
cargo run --locked \
  -p aegis-lock \
  --features dev-preview \
  --bin aegis-lock-preview \
  -- --composition cinematic --password 0000
```

Only the exact value `0000` closes this Preview; another nonempty value uses
the production rejection state and visual feedback. `--password` and
`--result` are mutually exclusive. Never pass a real account password:
command-line arguments may be retained in shell history and visible in the
process list. The fake value is local to this Preview process and is never
sent to PAM or saved in Aegis configuration.

## Select a Composition and Background

The Preview reads `[lock_screen]` from the normal Aegis configuration. Use a
scoped override when comparing layouts or reviewing artwork without editing
that file:

```bash
target/debug/aegis-lock-preview \
  --composition cinematic \
  --background /absolute/path/to/lock-screen.png
```

`--composition` accepts `cinematic` or `centered`; it selects the complete UI
composition rather than only visual styling. `--style` remains a compatibility
alias. `--background` accepts a static image and affects only that Preview
process. Production `aegis-lock` uses the independent
`[lock_screen.background]` configuration described in the
[Configuration Reference](../reference/config.md#lock-screen). An invalid
custom image falls back to the bundled lock artwork.

Capture both styles after changing shared typography, identity, credential,
or status rendering. For cinematic-specific work, include an empty-password
frame, a typing frame, and a rejected frame. The empty frame must have a
neutral rail with no password marks; the typing frame must show only marks for
entered characters, never empty slots that imply an expected password length;
the rejected frame must show a red rail without a text error. Use a short
recording rather than a still frame when reviewing the rejection shake.

## Select a Visual State

Use `--state` to hold a state long enough for inspection:

| State | Presentation |
|-------|--------------|
| `ready` | Engaged screen with a neutral, unmarked password rail |
| `typing` | Engaged screen with marks only for entered characters |
| `checking` | In-flight authentication indicator |
| `rejected` | Red credential rail and brief horizontal rejection shake |
| `unavailable` | Authentication-service failure message |
| `ambient` | Privacy-preserving idle presentation |

For example:

```bash
target/debug/aegis-lock-preview \
  --composition cinematic \
  --state rejected \
  --result rejected \
  --size 1280x800
```

`--result` controls the result of a later interactive submission:

| Result | Behavior |
|--------|----------|
| `accepted` | Close after a nonempty credential is submitted |
| `rejected` | Shake the credential rail briefly and hold its red error state |
| `unavailable` | Show the localized authentication-service message |

Use `--result` for a deterministic one-state inspection and `--password` for
an interactive accept/reject loop. They cannot be combined.

Interactive Preview submissions retain the checking state briefly so the
selected composition's validation feedback can be inspected. Cinematic uses
a fixed-rate cool light sweep from the credential rail's left edge to its
right. Its speed is independent of password length and authentication
backoff. The production lock keeps the same feedback active until PAM
replies. From submission until the result arrives, the shared lock state
freezes typing, deletion, clearing, and repeated submission while retaining
the entered marks as locked feedback. If Enter is pressed during rejection
backoff, the attempt remains pending and starts automatically at the retry
deadline; the same sweep continues across that transition instead of
restarting or silently discarding the key press.

Set the process locale to review translated lock copy:

```bash
LC_ALL=zh_CN.UTF-8 target/debug/aegis-lock-preview --state unavailable
```

Use `--size WIDTHxHEIGHT` for small, standard, ultrawide, and portrait layout
checks. The value is the fixed logical window size; the compositor's output
scale determines the physical buffer size.

## Capture Preview Pixels for an Agent

Keep the physical Aegis session unlocked. Start the Preview in one terminal,
then locate it through the outer compositor in another terminal:

```bash
preview_id=$(target/debug/aegis window -j | \
  jq -r 'map(select(.app_id == "dev.aegis.LockPreview")) | last | .id')

preview_region=$(target/debug/aegis window -j | \
  jq -r 'map(select(.app_id == "dev.aegis.LockPreview")) | last |
    "\(.position.x),\(.position.y),\(.size.w),\(.size.h)"')
```

Focus the Preview before capture. Running capture commands from a terminal can
otherwise raise that terminal above the target window:

```bash
target/debug/aegis window focus "$preview_id"
target/debug/aegis display capture \
  --region "$preview_region" \
  /tmp/aegis-lock-preview.png
```

The capture command queues asynchronous PNG encoding. Treat the image as ready
only after the destination exists and has a nonzero size. On HiDPI outputs the
PNG dimensions are the physical-pixel equivalent of the logical region.

The Preview logs `lock preview: first frame presented` after its first
successful render. Automation that must avoid parsing logs can pass an
inherited writable descriptor with `--ready-fd`; the process writes one byte
after that same frame.

## Test the Production Lock in a Nested Compositor

Use Preview results only for presentation. Validate session-lock behavior with
the unmodified production target inside nested Aegis.

Start the nested compositor:

```bash
AEGIS_BACKEND=nested cargo run --locked -p aegis
```

Copy the inner socket name from the startup log:

```text
server: listening on WAYLAND_DISPLAY=wayland-N
```

Build and start the production locker on that socket:

```bash
cargo build --locked \
  -p aegis-lock \
  --bin aegis-lock \
  --no-default-features

WAYLAND_DISPLAY=wayland-N target/debug/aegis-lock
```

The inner compositor is now genuinely locked. The unlocked outer compositor
can capture the nested Aegis window for review without weakening the inner
capture policy.

Enter the account password to test the complete PAM transition. For a visual
or protocol-only test, stop the entire nested compositor to recover. Do not
kill only `aegis-lock` after secure confirmation: the inner compositor must
retain its fail-closed frame.

Follow [Nested Backend Development](nested-backend.md) for socket discovery,
process isolation, and nested-backend limits.

## Test a Physical Session

Perform physical testing only after Preview and nested checks pass. Validate
the exact PAM service with `pamtester`, keep another TTY available, and trigger
the lock without waiting for the idle deadline:

```bash
aegis-idle --lock-now
```

Follow [How to Install and Verify the Lock Screen](../how-to/lock-screen.md)
for authentication and secure-transition checks. Follow
[VT/DRM Manual Testing](vt-drm-testing.md) for output, VT, suspend, and
hardware recovery checks.

## Select Tests by Change

| Change | Required checks |
|--------|-----------------|
| Layout, color, typography or localized copy | Preview both compositions at representative sizes, scales, locales and visual states |
| Independent lock background or artwork scrim | Preview with `--background`, then start a fresh production lock using `[lock_screen.background]` |
| Avatar image, VRM or animation | Preview initial frame, motion, hot reload and fallback avatar |
| Credential state machine or authentication feedback | `aegis-lock` unit tests plus Preview submission results |
| Lock-surface lifecycle or input routing | Nested lock, including client exit and outer capture |
| PAM service or account policy | `pamtester` followed by physical lock and unlock |
| Capture or ScreenCast policy | Nested policy check followed by physical-session verification |
| Idle, display-off, suspend or resume | Physical DRM/KMS session |

## Verify Production Exclusion

The Preview feature changes target availability, not production lock logic.
Build the production target explicitly in a fresh target directory:

```bash
aegis_prod_target=$(mktemp -d)
cargo build --locked --release \
  --target-dir "$aegis_prod_target" \
  -p aegis-lock \
  --bin aegis-lock \
  --no-default-features

test -x "$aegis_prod_target/release/aegis-lock"
test ! -e "$aegis_prod_target/release/aegis-lock-preview"
```

Distribution builds must not enable `dev-preview`, and install manifests must
name `target/release/aegis-lock` explicitly. Do not install binaries through a
`target/release/aegis-*` wildcard.

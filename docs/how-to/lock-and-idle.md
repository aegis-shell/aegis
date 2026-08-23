# How to Configure Locking and Idle

Aegis can dim the backlight, lock the session, turn displays off, and suspend
after increasing periods without keyboard, pointer, or touch activity.

## Lock Immediately

Press `Super+L`. Aegis covers every output and sends keyboard, pointer, and
touch input only to the lock screen. Enter the current account password and
press `Enter`, or select the arrow in the password field.

Before the first direct-session lock, follow
[How to Install and Verify the Lock Screen](lock-screen.md) to validate the
three sibling binaries and the host PAM service profile.

`Escape` or `Ctrl+U` clears entered text. The password also clears after 30
seconds without interaction. Rejected attempts use an increasing retry delay.

## Configure Automatic Actions

Open the command panel with `Super+S`, then select **Power Management**.

1. Turn **Automatic idle actions** on or off.
2. Enable the stages the session should use.
3. Choose an exact time for each enabled stage. The power settings keep later
   enabled stages after earlier stages.
4. Choose the dimmed brightness.
5. Select **Apply Power Settings**.

Turning automatic actions off preserves their configured times. Manual
locking and lock-before-sleep remain available.

## Choose a Session Power Mode

The idle stages apply as a set, but two concerns are often wanted
separately: whether the screen may blank, and whether the session may lock.
The command panel's Quick Controls (`Super+S`) expose both as switches —
**Keep Screen Awake** and **Automatic Lock** — which compose into the
session power mode (ADR-0140):

| Keep awake | Auto lock | Mode | Behavior after idle |
|------------|-----------|------|---------------------|
| off | on | `balanced` | Dim, lock, blank, suspend (the default) |
| on | on | `secure` | Dim and lock, but the display never blanks |
| either | off | `awake` | Dim only; nothing locks, blanks, or suspends |

The (off, off) combination is not offered: blanking or suspending an
unlocked session is forbidden, so turning **Automatic Lock** off keeps the
display awake regardless, and the switch reads that back. The mode is
session-scoped — it resets to `balanced` on the next session — and manual
locking (`Super+L`) plus lock-before-sleep apply in every mode. Switching
modes takes effect immediately, without restarting the idle coordinator.

The CLI equivalent is `aegis system power-mode <mode>`.

## Configure the Policy as TOML

Edit `$XDG_CONFIG_HOME/aegis/config.toml`:

```toml
[idle]
enabled = true
dim_after_seconds = 300
lock_after_seconds = 600
display_off_after_seconds = 660
suspend_after_seconds = 1800
dim_percent = 30
```

Set an individual timeout to `0` to disable that stage. Enabled timeouts must
increase in this order: dim, lock, display off, suspend. Display off and
suspend require a nonzero lock timeout. Saving the file replaces the running
idle policy without restarting the compositor.

See the [Configuration Reference](../reference/config.md#idle-and-locking)
for exact bounds and defaults.

## Understand Session Differences

A direct DRM/KMS session owns the physical backlight, displays, and system
sleep transition. A nested Aegis session locks inside its own window but
leaves brightness, display power, and suspend to the outer desktop.

Display power-off and suspend wait until the compositor has confirmed a
secure lock. Input wakes powered-down displays behind that lock; it does not
expose the desktop.

See the [Session Service Commands](../reference/session-services.md) for
direct invocation and daemon options.

For PAM setup, safe first-lock testing, recovery, and avatars, use
[How to Install and Verify the Lock Screen](lock-screen.md).

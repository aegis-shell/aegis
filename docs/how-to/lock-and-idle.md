# How to Configure Locking and Idle

Aegis can dim the backlight, lock the session, turn displays off, and suspend
after increasing periods without keyboard, pointer, or touch activity.

## Lock Immediately

Press `Super+L`. Aegis covers every output and sends keyboard, pointer, and
touch input only to the lock screen. Enter the current account password and
press `Enter`, or select the arrow in the password field.

`Escape` or `Ctrl+U` clears entered text. The password also clears after 30
seconds without interaction. Rejected attempts use an increasing retry delay.

## Configure Automatic Actions

Open **System Settings**, then select **Power Management**.

1. Turn **Automatic idle actions** on or off.
2. Enable the stages the session should use.
3. Choose an exact time for each enabled stage. System Settings keeps later
   enabled stages after earlier stages.
4. Choose the dimmed brightness.
5. Select **Apply Power Settings**.

Turning automatic actions off preserves their configured times. Manual
locking and lock-before-sleep remain available.

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

If the lock screen reports that authentication is unavailable, the installed
package may be missing a working `aegis-lock` PAM service profile, or the
host authentication stack may be unavailable, busy, or timed out. Wait for
an in-progress attempt to finish before retrying.

See the [Session Service Commands](../reference/session-services.md) for
direct invocation and daemon options.

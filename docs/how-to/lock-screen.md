# How to Install and Verify the Lock Screen

Install and validate the complete lock path before relying on automatic
display power-off or suspend. Aegis splits this path across the compositor,
an idle coordinator, a lock-screen client, and the host authentication
stack. Installing `aegis-lock` alone does not enable the feature.

## Identify the Security Boundaries

| Component | Responsibility |
|-----------|----------------|
| `aegis` | Owns `ext-session-lock-v1`, hides normal content, routes input only to lock surfaces, and keeps a fail-closed frame after a locker failure. |
| `aegis-idle` | Applies the dim, lock, display-off, and suspend policy and starts the trusted sibling `aegis-lock`. |
| `aegis-lock` | Renders one lock surface per output, holds the credential briefly, and calls PAM for the current account. |
| `/etc/pam.d/aegis-lock` | Selects the host authentication and account policy used to unlock. |
| `brightnessctl` | Dims and restores a physical backlight when host permissions allow it. |
| `systemd-logind` | Supplies the sleep delay inhibitor, sleep notifications, and suspend request in a direct session. |
| `pam_aegis.so` | Optionally caches a successful password for portal secret-vault auto-unlock. It is not the screen authenticator. |

Unlocking does not run `aegis-lock` as root and does not use a compositor
password database. PAM applies the host's current authentication and account
policy. Suspend is a separate system D-Bus operation and can be allowed or
denied by the host's logind and authorization policy.

## Install the Complete Core Set

Install `aegis`, `aegis-idle`, and `aegis-lock` from the same package and
release. The three executables must be siblings, normally under `/usr/bin`.
The compositor resolves `aegis-idle` beside its own executable, and the idle
coordinator resolves `aegis-lock` beside itself; neither path searches
`PATH` for a replacement.

The core package must also install:

```text
/etc/pam.d/aegis-lock
```

The supplied profile uses the host's `login` authentication and account
classes:

```text
auth include login
auth optional pam_aegis.so
account include login
```

A distribution can replace `login` with its canonical system authentication
stack, but it must preserve both the `auth` and `account` classes. Keep
`pam_aegis.so` optional and after the primary authentication include.

### Prepare a source-tree test

Build all three programs into the same Cargo target directory:

```bash
cargo build --locked --release -p aegis -p aegis-idle -p aegis-lock
```

Install the PAM service profile before testing authentication:

```bash
sudo install -Dm0644 contrib/pam/aegis-lock /etc/pam.d/aegis-lock
```

Run `target/release/aegis`; it discovers the two sibling programs without a
system installation. Do not make any Aegis executable setuid.

## Validate PAM Before Locking

Confirm that the service file exists and that all three binaries are from the
same installation:

```bash
test -r /etc/pam.d/aegis-lock
sed -n '1,80p' /etc/pam.d/aegis-lock
readlink -f /usr/bin/aegis /usr/bin/aegis-idle /usr/bin/aegis-lock
```

When `pamtester` is available, test the exact service outside the lock screen:

```bash
pamtester aegis-lock "$USER" authenticate
pamtester aegis-lock "$USER" acct_mgmt
```

The first command must prompt for the current account password and accept the
correct value. The second must accept the account policy. Fix a failure before
locking a direct session.

For a source build, perform the first UI test in a nested Aegis window. A PAM
failure then locks only that nested compositor rather than the outer desktop.
For a direct DRM session, keep access to another TTY until authentication has
been verified.

## Lock the Session

Use any one of these entry points:

- press `Super+L`;
- select **Lock Now** in the command panel; or
- run `aegis-idle --lock-now` inside the Aegis session environment.

Running `aegis-lock` directly is useful for diagnosis, but normal sessions
should go through the supervised idle coordinator.

Wait for the lock screen to cover every output. Enter the current account
password and press `Enter`, or select the arrow in the password field.
`Escape` and `Ctrl+U` clear the credential. The credential also clears after
30 seconds without interaction, and rejected attempts use an increasing
retry delay.

## Verify the Secure Transition

The display-off and suspend stages begin only after `aegis-lock` has rendered
every output and the compositor has confirmed the secure presentation. Check
the packaged session journal for the complete handshake:

```bash
journalctl --user -b -u aegis.service --no-pager | \
  grep -E 'session lock|secure|authentication|idle:'
```

Confirm these behaviors:

1. All outputs show lock content before any display powers off.
2. Keyboard, pointer, and touch input affect only the lock screen.
3. A portal screenshot is refused and an existing ScreenCast pauses.
4. Input wakes powered-down displays behind the lock without showing the
   desktop.
5. A successful PAM result unlocks every output together.

If the lock client exits after confirmation, the compositor intentionally
does not reveal the session. It retains an opaque fail-closed frame. Do not
test crash behavior until TTY recovery is available.

## Troubleshoot Authentication and Permissions

| Symptom | Meaning and action |
|---------|--------------------|
| *Authentication misconfigured · Install the aegis-lock PAM profile* | PAM rejected the attempt without requesting a password. Install or repair `/etc/pam.d/aegis-lock`, then retry. Retyping the password cannot fix this condition. |
| *Incorrect password* | PAM requested a credential and rejected it. Check the password, keyboard layout, Caps Lock state, and the host authentication policy. |
| *Authentication is still running* | One PAM conversation is already active. Wait for it to complete; concurrent attempts are refused. |
| *Authentication timed out* | The UI watchdog waited 30 seconds. Check slow or unavailable PAM modules and network-backed account services. |
| The lock shortcut logs that a trusted sibling is missing | Install `aegis-idle` and `aegis-lock` beside the running `aegis` executable. A copy found elsewhere on `PATH` is intentionally ignored. |
| Locking works but dimming does not | Confirm `brightnessctl --class=backlight get` works for the session user. The lock policy continues when backlight access is unavailable. |
| Locking works but suspend does not | Confirm a direct session can call logind and that host authorization permits suspend. Inspect the journal for `logind suspend request failed`. |
| Only locking works in a nested session | This is expected. The outer desktop retains physical backlight, output-power, and suspend authority. |
| `pam_aegis.so` is missing | Screen authentication should still work because the module is optional. Only portal secret-vault auto-unlock is unavailable. |

## Recover a Misconfigured Direct Session

Switch to another TTY, sign in as the same account, and repair
`/etc/pam.d/aegis-lock`. PAM loads the service policy for a new attempt, so
return to the locked VT and retry without killing the lock client.

If the authentication service remains permanently blocked, end the complete
Aegis session from the recovery TTY and sign in again after fixing the host
policy. Killing only `aegis-lock` cannot unlock the confirmed session and can
leave it deliberately fail-closed.

## Set a Lock-Screen Avatar

Place a still image in the Aegis data namespace. The first decodable file in
this order is used:

- `$XDG_DATA_HOME/aegis/avatars/face.png`
- `$XDG_DATA_HOME/aegis/avatars/face.jpg`
- `$XDG_DATA_HOME/aegis/avatars/face.webp`
- `$XDG_DATA_HOME/aegis/avatars/face`
- `~/.face`
- `~/.face.icon`

PNG, JPEG, WebP, GIF, BMP, ICO, TIFF, TGA, QOI, and PNM images are accepted.
The image is cover-fit and circle-masked. An invalid or absent image falls
back to the built-in gradient identity disc.

A VRM 0.x or 1.0 model can instead be placed at:

- `$XDG_DATA_HOME/aegis/avatars/avatar.vrm`

To animate it, add the companion VRM Animation 1.0 clip at:

- `$XDG_DATA_HOME/aegis/avatars/avatar.vrma`

A still image takes precedence over a VRM model. Aegis retargets the VRMA
humanoid motion onto VRM 0.x or 1.0 bones, loops it while the avatar is
visible, and keeps the animated head and shoulders inside the identity disc.
Without the companion clip, the VRM remains in its rest pose.

See [How to Configure Locking and Idle](lock-and-idle.md) for automatic
timeouts and the [Session Service Commands](../reference/session-services.md)
for direct command options and exit behavior.

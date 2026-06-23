# Configuration Reference

Exact reference for the ass configuration file at
`$XDG_CONFIG_HOME/ass/config.toml` (defaulting to `~/.config/ass/config.toml`).
For the design behind it, the loader, and the live-reload contract, see
[ADR-0026](../adr/0026-configuration-system.md).

The file is TOML. It is hot-reloaded: save it and behavior changes without a
restart. A malformed file or an unknown field is reported as a `config:`
log diagnostic and never crashes the compositor.

## Top-Level Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `schema_version` | integer | required | Schema major version. Must be `1`. A different value is rejected with a diagnostic; bumping it is a documented migration event. |
| `[[keybind]]` | array of tables | built-in defaults | Global key bindings. See [Key Bindings](#key-bindings). |
| `[[window_rule]]` | array of tables | none | Placement rules applied to newly-mapped toplevels. See [Window Rules](#window-rules). |

## Window Rules

Each `[[window_rule]]` table matches a toplevel when it first maps and
prescribes a placement action. The first matching rule applies; a rule with
no matchers matches nothing.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `app_id` | string | — | Match if the toplevel's `app_id` contains this, case-insensitively. |
| `title` | string | — | Match if the toplevel's `title` contains this, case-insensitively. |
| `workspace` | integer | — | Move the window to this 1-based workspace index on the focused output. Applied only if that workspace exists. |
| `role` | string | — | Force the layout role: `floating` or `tiled`. A `floating` window is exempt from tiling. |

When both `app_id` and `title` are set, both must match (AND). Rules apply at
first map; an `app_id`/`title` set after mapping is not re-evaluated yet.

```toml
[[window_rule]]
app_id = "firefox"
workspace = 2
role = "tiled"

[[window_rule]]
title = "calculator"
role = "floating"
```

## Key Bindings

Each `[[keybind]]` table binds a modifier set plus a key to an action.
Bindings layer over the [built-in defaults](#default-key-bindings): a file
with one binding keeps the rest. Several `[[keybind]]` tables may bind the
same action to different keys.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `mods` | array of strings | `[]` | Modifier names, OR'd together. See [Modifier Names](#modifier-names). |
| `key` | string | required | Key name. See [Key Names](#key-names). |
| `action` | string | required | Action name. See [Action Names](#action-names). |

### Modifier Names

| Name | Aliases | Bit |
|------|---------|-----|
| `shift` | | Shift |
| `ctrl` | `control` | Control |
| `alt` | `mod1` | Mod1 (Alt) |
| `super` | `meta`, `win`, `mod4` | Mod4 (Super/Logo) |

Matching is exact on the depressed modifier mask: `mods = ["super"]` does
not also fire when Ctrl is held. Lock state (CapsLock, NumLock) does not
affect matching.

### Key Names

Letters (`a`–`z`, lowercased), digits (`0`–`9`), and the common controls:

| Name | Key |
|------|-----|
| `space` | Space |
| `return`, `enter` | Return / Enter |
| `escape`, `esc` | Escape |
| `tab` | Tab |
| `backspace`, `bs` | Backspace |
| `up`, `down`, `left`, `right` | Arrows |
| `home`, `end` | Home, End |
| `pageup`, `pgup`, `pagedown`, `pgdn` | Page Up / Down |
| `delete`, `del` | Delete |
| `f1` … `f12` | Function keys |

### Action Names

| Name | Aliases | Effect |
|------|---------|--------|
| `launcher` | `togglelauncher`, `apps` | Open or close the application launcher |
| `close` | `closefocused` | Close the focused toplevel |
| `cycle` | `next` | Move focus to the next toplevel |
| `prev` | `previous`, `cycleback` | Move focus to the previous toplevel |
| `quit` | `exit` | Quit the compositor |

A matched binding is consumed before delivery to the focused client, so the
client never sees the key that triggered it.

### Default Key Bindings

These ship as built-in defaults and remain in effect when no override is
configured:

| Binding | Action |
|---------|--------|
| `Super+Tab` | `cycle` |
| `Super+Shift+Tab` | `prev` |
| `Super+Return` | `launcher` |
| `Super+Q` | `close` |
| `Super+Shift+Return` | `quit` |

A bare Super tap (press and release with no other key in between) also
toggles the launcher; it is detected separately and is not a `[[keybind]]`.

## Example

```toml
schema_version = 1

[[keybind]]
mods = ["super"]
key = "space"
action = "launcher"

[[keybind]]
mods = ["super", "shift"]
key = "q"
action = "quit"

[[keybind]]
mods = ["super"]
key = "right"
action = "cycle"
```

## Migration from `$ASS_KEYBINDS`

The `$ASS_KEYBINDS` environment variable (`mods+key=action;...`) is
deprecated and remains honored as a transitional override that takes
precedence over the file. To migrate, move each entry into a `[[keybind]]`
table. The variable will be removed before the desktop phase closes.

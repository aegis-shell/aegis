# Tutorial: Getting Started with ASS

Welcome to the **ass** (*autonomous surface shell*) getting started tutorial. This walkthrough will guide you through setting up your development environment, building the compositor, running your first nested session, launching standalone components, and testing IPC interaction.

---

## Prerequisites

Before starting, ensure your Linux system has the following build tools installed:

- **Rust toolchain** (Rust 1.88+ / 2024 edition)
- **Meson** and **Ninja**
- **Pkg-config**
- **Bash 4.3+**
- System C headers for **Wayland** (`libwayland-dev`, `wayland-protocols`), **Vulkan**, and **xkbcommon**

---

## Step 1: Building the Optics C Libraries

`ass` depends on the `flux` rendering engine and `lens` UI library located in the sibling `../optics` directory.

Compile the optics C libraries first:

```bash
meson setup ../optics/build ../optics -Dtests=false -Dbuildtype=debugoptimized
meson compile -C ../optics/build
```

*(Note: Skip `meson setup` if `../optics/build` has already been initialized.)*

---

## Step 2: Running the Compositor in Nested Mode

Run the integrated development command inside an existing Wayland session:

```bash
scripts/dev.sh
```

The runner builds `aegis` and `aegis-settings`, stages System Settings under
`target/aegis-dev`, and launches one compositor session in a nested window.
The desktop background, status bar, and Dock appear inside that window.

---

## Step 3: Launching System Settings

Open Applications inside the nested session and select **System Settings**.
The launcher discovers its staged desktop entry and icon, starts the
independent process, and connects it to the nested compositor's Wayland and
owner-only IPC sockets.

The Settings window appears as its own application and groups under the
System Settings Dock identity.

---

## Step 4: Interacting via the Control CLI

The development command prints its private runtime directory at startup.
Copy that path, then query compositor state from a second terminal:

```bash
aegis_runtime=/run/user/1000/aegis-dev.ABC123
XDG_RUNTIME_DIR="$aegis_runtime" cargo run -p aegis-ctl -- get outputs
XDG_RUNTIME_DIR="$aegis_runtime" cargo run -p aegis-ctl -- get windows
```

Replace the example value with the directory printed by
`scripts/dev.sh`. Both commands return state from the nested compositor.

## Next Steps

- Check out the [Daily-use How-To Guides](../how-to/index.md) for keybindings, window management, and display configuration.
- Read the [Architecture Overview](../explanation/architecture.md) to learn about the internal design of `ass`.

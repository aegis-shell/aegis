# Tutorial: Getting Started with ASS

Welcome to the **ass** (*autonomous surface shell*) getting started tutorial. This walkthrough will guide you through setting up your development environment, building the compositor, running your first nested session, launching standalone components, and testing IPC interaction.

---

## Prerequisites

Before starting, ensure your Linux system has the following build tools installed:

- **Rust toolchain** (Rust 1.88+ / 2024 edition)
- **Meson** and **Ninja**
- **Pkg-config**
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

Run `ass` inside your existing X11 or Wayland session:

```bash
cargo run -p ass
```

This launches the compositor in a nested window on your display. You should see the desktop background, statusbar, and dock appear inside the nested window.

---

## Step 3: Launching the Control Center

Open a second terminal window and run the standalone settings app:

```bash
cargo run -p aegis-ctl-center
```

The Control Center connects to the running compositor over its owner-only IPC socket, allowing you to configure display settings, input preferences, and themes interactively.

---

## Step 4: Interacting via the Control CLI

You can query compositor state and send commands using `aegis-ctl`:

```bash
# Query active outputs and windows
cargo run -p aegis-ctl -- get outputs
cargo run -p aegis-ctl -- get windows

# Subscribe to compositor events
cargo run -p aegis-ctl -- subscribe
```

---

## Next Steps

- Check out the [Daily-use How-To Guides](../how-to/index.md) for keybindings, window management, and display configuration.
- Read the [Architecture Overview](../explanation/architecture.md) to learn about the internal design of `ass`.

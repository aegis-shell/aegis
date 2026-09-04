{
  description = "Tessera desktop compositor and Wayland window manager";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "rustfmt" "clippy" "rust-analyzer" ];
        };

        nativeBuildInputs = with pkgs; [
          pkg-config
          meson
          ninja
          mold
          sccache
          rustToolchain
          llvmPackages.clang
          llvmPackages.libclang
          glslang
          wayland-scanner
        ];

        buildInputs = with pkgs; [
          wayland
          wayland-protocols
          vulkan-headers
          vulkan-loader
          vulkan-validation-layers
          mesa
          libxkbcommon
          libinput
          libseat
          udev
          pam
          systemd
          freetype
          harfbuzz
          fontconfig
          fribidi
          glfw
        ];
      in
      {
        devShells.default = pkgs.mkShell {
          inherit nativeBuildInputs buildInputs;

          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
          RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
          RUSTFLAGS = "-C symbol-mangling-version=v0 -C link-arg=-fuse-ld=mold";
          SCCACHE_GHA_ENABLED = "true";

          shellHook = ''
            export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath buildInputs}:$LD_LIBRARY_PATH"
          '';
        };
      });
}

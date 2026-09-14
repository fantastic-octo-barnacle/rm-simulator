# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
{
  description = "rm-simulator native development dependencies";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
  outputs =
    { nixpkgs, ... }:
    let
      systems = [
        "aarch64-darwin"
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      devShells = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              rustup
              cmake
              ninja
              pkg-config
              clang
              libclang
              protobuf_21
              openssl
              just
              python3
              cargo-deny
              prek
            ];
            buildInputs =
              with pkgs;
              lib.optionals stdenv.hostPlatform.isLinux [
                alsa-lib
                udev
                vulkan-loader
                libxkbcommon
                wayland
                libX11
                libXcursor
                libXi
                libXrandr
              ];
            LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
            # Keep Nix native builds separate from existing Homebrew-linked objects.
            shellHook = ''
              export CARGO_TARGET_DIR="$PWD/target/nix"
            '';
          };
        }
      );
    };
}

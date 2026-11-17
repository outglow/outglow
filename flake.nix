{
  # `nix develop`: the shell CI runs in. The firmware image depends on the
  # LLVM inside rustc (rustc emits the bitcode llvm-tc32 compiles), so the
  # same nixpkgs everywhere means the same image on every machine;
  # `cargo xtask build` warns when another rustc builds it.
  #
  # `nix develop .#coverage`: line coverage of the host tests (cargo llvm-cov,
  # `cargo xtask coverage`). It has the official Rust binaries of the same
  # version (rust-overlay) with their llvm-tools: nixpkgs' rustc instruments
  # the code with LLVM 21, but its profiler runtime writes raw profiles of a
  # newer format that no llvm-profdata of LLVM 21 reads, and on aarch64-darwin
  # its instrumented test binaries crash at exit. The official toolchain ships
  # the llvm-profdata and llvm-cov of its own LLVM. Not for firmware images:
  # its LLVM is another one, and so would be the image.
  description = "Outglow: open firmware for Telink TLSR8258 Zigbee bulbs";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/1c89083e451e891152365b799595cf7790a43325";
  inputs.rust-overlay = {
    url = "github:oxalica/rust-overlay/2ff7d39b901d79ee26d26f9fa32f90511eecaa0e";
    inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { nixpkgs, rust-overlay, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" "x86_64-darwin" ];
      each = f: nixpkgs.lib.genAttrs systems (system: f (import nixpkgs {
        inherit system;
        overlays = [ rust-overlay.overlays.default ];
      }));
    in
    {
      devShells = each (pkgs:
        let
          common = with pkgs; [
            python3 # tc32-devtools' checks of rust_fw
            nodejs_22 # the website, the Zigbee2MQTT converter check
            pkg-config
            cargo-mutants
          ] ++ lib.optionals stdenv.hostPlatform.isLinux [ dbus ] # btleplug
            ++ lib.optionals stdenv.hostPlatform.isDarwin [ libiconv ];
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [ rustc cargo ] ++ common;
            # build-std's sources: the standard library of this very rustc
            RUST_LIB_SRC = "${pkgs.rustPlatform.rustLibSrc}";
          };
          coverage = pkgs.mkShell {
            packages = [
              (pkgs.rust-bin.stable."1.99.0".minimal.override { extensions = [ "llvm-tools" ]; })
              # (its own tests need a network and a rustup toolchain)
              (pkgs.cargo-llvm-cov.overrideAttrs { doCheck = false; })
            ] ++ common;
          };
        });
    };
}

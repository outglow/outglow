{
  # `nix develop`: the shell CI runs in. The firmware image depends on the
  # LLVM inside rustc (rustc emits the bitcode llvm-tc32 compiles), so the
  # same nixpkgs everywhere means the same image on every machine;
  # `cargo xtask build` warns when another rustc builds it.
  description = "Outglow: open firmware for Telink TLSR8258 Zigbee bulbs";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/1c89083e451e891152365b799595cf7790a43325";

  outputs = { nixpkgs, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" "x86_64-darwin" ];
      each = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      devShells = each (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            rustc
            cargo
            python3 # tc32-devtools' checks of rust_fw
            nodejs_22 # the website, the Zigbee2MQTT converter check
            pkg-config
          ] ++ lib.optionals stdenv.hostPlatform.isLinux [ dbus ] # btleplug
            ++ lib.optionals stdenv.hostPlatform.isDarwin [ libiconv ];
          # build-std's sources: the standard library of this very rustc
          RUST_LIB_SRC = "${pkgs.rustPlatform.rustLibSrc}";
        };
      });
    };
}

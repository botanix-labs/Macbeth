
let
  rustOverlay = import (builtins.fetchTarball "https://github.com/oxalica/rust-overlay/archive/master.tar.gz");
  pkgs = import <nixpkgs> { overlays = [ rustOverlay ]; };
  #rustVersion = "latest";
  rustVersion = "1.77.2";
  rust = pkgs.rust-bin.stable.${rustVersion}.default.override {
    extensions = [ "rust-analyzer" "rust-src" ];
  };
in
pkgs.mkShell {
  buildInputs = [ rust ] ++ (with pkgs; [
    pkg-config
    openssl
    glibc
    clang
    libclang
    protobuf
    rustc
    cargo
  ]);

  # PROJECT_ROOT = builtins.toString ./.;
  RUST_BACKTRACE = 1;
  RUST_SRC_PATH = "${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";
  LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
}

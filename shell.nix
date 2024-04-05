{ pkgs ? import <nixpkgs> {} }:

pkgs.mkShell {
  buildInputs = with pkgs; [
    pkg-config
    openssl
    glibc
    clang
    libclang
    protobuf
  ];

  LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
}

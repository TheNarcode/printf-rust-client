let
  pkgs = import (fetchTarball "channel:nixpkgs-unstable") {};
in
pkgs.callPackage (
  {
    mkShell,
    cargo,
    rustc,
    openssl,
    pkg-config,
  }:
  mkShell {
    strictDeps = true;

    nativeBuildInputs = [
      cargo
      rustc
      pkg-config
    ];

    buildInputs = [
      openssl
    ];

    OPENSSL_DIR = "${openssl.dev}";
    OPENSSL_LIB_DIR = "${openssl.out}/lib";
    OPENSSL_INCLUDE_DIR = "${openssl.dev}/include";
  }
) {}

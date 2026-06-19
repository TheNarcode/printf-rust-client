{ pkgs, lib, config, inputs, ... }:

{
  packages = [
    pkgs.pkg-config
    pkgs.openssl
  ];

  languages = {
    rust = {
      enable = true;
    };
  };

  env = {
    OPENSSL_DIR = "${pkgs.openssl.dev}";
    OPENSSL_LIB_DIR = "${pkgs.openssl.out}/lib";
    OPENSSL_INCLUDE_DIR = "${pkgs.openssl.dev}/include";
  };
}

let
  pkgs = import <nixpkgs> { };
in
pkgs.mkShell {
  nativeBuildInputs = with pkgs; [
    pkg-config
    wrapGAppsHook4

    cargo
    cargo-tauri
    nodejs
    rustc

    # Skia build requirements
    python3
    ninja
    clang
    llvmPackages.libclang
    cmake
    git
    perl
  ];

  buildInputs = with pkgs; [
    librsvg
    webkitgtk_4_1

    openssl
    glib
    gtk3
    libsoup_3
    xdotool

    # Skia dependencies
    freetype
    fontconfig
    harfbuzz
    icu
    libpng
    libjpeg
    zlib
    expat
    bzip2
  ];

  OPENSSL_DIR = "${pkgs.openssl.dev}";
  OPENSSL_LIB_DIR = "${pkgs.openssl.out}/lib";
  OPENSSL_INCLUDE_DIR = "${pkgs.openssl.dev}/include";

  shellHook = ''
    export XDG_DATA_DIRS="$GSETTINGS_SCHEMAS_PATH"

    export PKG_CONFIG_PATH="
      ${pkgs.openssl.dev}/lib/pkgconfig:
      ${pkgs.webkitgtk_4_1.dev}/lib/pkgconfig:
      ${pkgs.libsoup_3.dev}/lib/pkgconfig:
      ${pkgs.gtk3.dev}/lib/pkgconfig:
      ${pkgs.glib.dev}/lib/pkgconfig:
      ${pkgs.freetype.dev}/lib/pkgconfig:
      ${pkgs.fontconfig.dev}/lib/pkgconfig:
      ${pkgs.harfbuzz.dev}/lib/pkgconfig:
      $PKG_CONFIG_PATH
    "

    # Compiler
    export CC="${pkgs.clang}/bin/clang"
    export CXX="${pkgs.clang}/bin/clang++"

    # Required by bindgen
    export LIBCLANG_PATH="${pkgs.llvmPackages.libclang.lib}/lib"

    # Helps clang find headers on NixOS
    export BINDGEN_EXTRA_CLANG_ARGS="-I${pkgs.glib.dev}/include -I${pkgs.icu.dev}/include"

    echo "Using clang:"
    clang --version

    echo "Using libclang:"
    ls $LIBCLANG_PATH/libclang.so*
  '';
}

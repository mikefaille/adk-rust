# =============================================================================
# ADK-Rust Development Environment (devenv.nix)
# =============================================================================
# Optimized for monorepo scale to fix "Argument list too long" (ARG_MAX) errors.
# =============================================================================

{ pkgs, lib, config, ... }:

let
  llvm = pkgs.llvmPackages_latest;
  
  # Consolidated environment to fix "Argument list too long" (ARG_MAX) errors
  # by replacing dozens of individual store paths with a single search path.
  adkBuildEnv = pkgs.buildEnv {
    name = "adk-build-env";
    paths = [
      pkgs.pkg-config
      pkgs.openssl
      pkgs.cmake
      pkgs.protobuf
      pkgs.glib
      pkgs.glib.dev
      pkgs.libva
      pkgs.libvdpau
      pkgs.libxcb
      pkgs.libX11
      pkgs.libXcursor
      pkgs.libXext
      pkgs.libXi
      pkgs.libXrender
      pkgs.libxkbcommon
      pkgs.fontconfig
      pkgs.freetype
      pkgs.pipewire
      pkgs.wayland
      pkgs.dbus
      pkgs.libcap
      pkgs.systemd
      # Lower priority for xorgproto to avoid header collisions with libX11
      (lib.lowPrio pkgs.xorg.xorgproto)
    ];
  };

in {
  name = "adk-rust";

  # Enable Cachix binary cache
  cachix.pull = [ "devenv" ];

  # Load .env file automatically

  # https://devenv.sh/packages/
  packages = [ 
    pkgs.git
    pkgs.jq
    pkgs.curl
    pkgs.nodejs_22
    pkgs.bun
    pkgs.sccache
    pkgs.mold
    pkgs.wild
    
    # System libraries (redundant but safe for pkg-config)
    pkgs.glib
    pkgs.glib.dev
    pkgs.libva
    
    # Core build environment
    adkBuildEnv
    
    # LLVM toolchain
    llvm.clang
    llvm.libclang
    llvm.lld
  ] ++ lib.optionals pkgs.stdenv.isLinux [
    pkgs.valgrind
  ];

  # Centralized Build Search Paths
  env.CPATH = "${adkBuildEnv}/include";
  env.LIBRARY_PATH = "${adkBuildEnv}/lib";
  env.PKG_CONFIG_PATH = "${adkBuildEnv}/lib/pkgconfig";
  
  # Rust/LLVM configuration
  env.LIBCLANG_PATH = "${llvm.libclang.lib}/lib";
  env.PROTOC = "${adkBuildEnv}/bin/protoc";
  env.CC = "clang";
  env.CXX = "clang++";
  env.LD = "lld";
  
  # Optimization
  env.RUSTC_WRAPPER = "sccache";
  env.SCCACHE_CACHE_SIZE = "50G";
  env.WILD_INCREMENTAL = "1";
  env.CMAKE_POLICY_VERSION_MINIMUM = "3.5";

  # https://devenv.sh/languages/
  languages.rust = {
    enable = true;
    channel = "stable";
    components = [ "rustc" "cargo" "clippy" "rustfmt" "rust-analyzer" ];
  };

  # https://devenv.sh/scripts/
  scripts.ws-fmt.exec = "cargo fmt --all $@";
  scripts.ws-check.exec = "cargo check --all-features $@";
  scripts.ws-test.exec = "cargo test --all-features $@";
  scripts.ws-clippy.exec = "cargo clippy --all-features -- -D warnings $@";

  # https://devenv.sh/pre-commit-hooks/
  git-hooks.hooks = {
    rustfmt.enable = true;
    clippy.enable = true;
    clippy.settings.allFeatures = true;
  };

  enterShell = ''
    echo "🚀 Welcome to the ADK-Rust Development Environment!"
    echo "   Rust:    $(rustc --version)"
    echo "   Cargo:   $(cargo --version)"
    echo "   sccache: $(sccache --version 2>/dev/null || echo 'not found')"
    echo "   Node:    $(node --version)"
    echo ""
    echo "💡 Run 'devenv tasks list' or use the scripts: ws-fmt, ws-check, ws-test, ws-clippy."
  '';
}

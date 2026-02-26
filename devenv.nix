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
      pkgs.glib
      pkgs.libopus
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
  # https://devenv.sh/basics/
  # env.GREET = "devenv";

  # https://devenv.sh/packages/
  packages = [ 
    pkgs.git
    pkgs.curl
    pkgs.jq
    pkgs.nodejs_22
    pkgs.bun
    pkgs.sccache
    pkgs.livekit-cli
    
    # Core build environment
    adkBuildEnv
    
    # LLVM toolchain
    llvm.clang
    llvm.libclang
    llvm.lld
  ];

  # Centralized Build Search Paths
  env.CPATH = "${adkBuildEnv}/include";
  env.LIBRARY_PATH = "${adkBuildEnv}/lib";
  env.PKG_CONFIG_PATH = "${adkBuildEnv}/lib/pkgconfig";
  
  # Rust/LLVM configuration
  env.LIBCLANG_PATH = "${llvm.libclang.lib}/lib";
  env.CC = "clang";
  env.CXX = "clang++";
  env.LD = "lld";
  
  # Sccache optimization
  env.RUSTC_WRAPPER = "sccache";
  env.SCCACHE_CACHE_SIZE = "50G";

  # https://devenv.sh/languages/
  languages.rust = {
    enable = true;
    channel = "stable";
    components = [ "rustc" "cargo" "clippy" "rustfmt" "rust-analyzer" ];
  };

  # https://devenv.sh/scripts/
  scripts.fmt.exec = "cargo fmt --all";
  scripts.check.exec = "cargo check --all-features";
  scripts.test.exec = "cargo test --all-features";
  scripts.clippy.exec = "cargo clippy --all-features -- -D warnings";

  # https://devenv.sh/pre-commit-hooks/
  pre-commit.hooks = {
    rustfmt.enable = true;
    clippy.enable = true;
    clippy.settings.allFeatures = true;
  };

  # https://devenv.sh/processes/
  # processes.ping.exec = "ping devenv.sh";

  # See full reference at https://devenv.sh/reference/options/
}

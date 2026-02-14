{ pkgs, lib, config, inputs, ... }:

{
  # 1. Enable Rust Nightly with Cranelift
  languages.rust = {
    enable = true;
    channel = "nightly";
    components = [
      "rustc"
      "cargo"
      "rustc-codegen-cranelift-preview" # <--- The speed booster
    ];
  };

  # 2. System Tools & Libraries
  packages = [
    pkgs.wild           # Fast Linker
    pkgs.sccache        # Compiler Cache
    pkgs.pkg-config     # Library Finder
    pkgs.glib
    pkgs.glib.dev       # <--- CRITICAL: Fixes "glib-2.0 not found"
  ];

  # 3. Environment Configuration
  env.RUSTC_WRAPPER = "sccache";

  # Force pkg-config to look in the glib.dev output
  enterShell = ''
    export PKG_CONFIG_PATH="${pkgs.glib.dev}/lib/pkgconfig:$PKG_CONFIG_PATH"

    echo "🏎️  Rust Ferrari Mode (Minimal): Nightly + Cranelift + Wild + Sccache"
  '';
}

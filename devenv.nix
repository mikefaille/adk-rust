{ pkgs, lib, config, inputs, ... }:

{
  # 1. Enable Rust Nightly with Cranelift
  languages.rust = {
    enable = true;
    channel = "nightly";
    version = "2026-01-15"; # <--- The Safe Harbor
    components = [
      "rustc"
      "cargo"
      "clippy"
      "rustfmt"
      "rust-analyzer"
      "rust-src"
      "rustc-codegen-cranelift-preview" # <--- The speed booster
    ];
  };

  # 2. System Tools (Linker & Cache) & 3. Libraries (Runtime + Headers)
  packages = [
    pkgs.wild           # Fast Linker
    pkgs.sccache        # Compiler Cache
    pkgs.pkg-config     # Library Finder
    pkgs.glib
    pkgs.glib.dev       # <--- CRITICAL: Fixes "glib-2.0 not found"
  ];

  # 4. Environment Configuration
  env.RUSTC_WRAPPER = "sccache";

  # Force pkg-config to look in the glib.dev output
  enterShell = ''
    export PKG_CONFIG_PATH="${pkgs.glib.dev}/lib/pkgconfig:$PKG_CONFIG_PATH"

    echo "🏎️  Rust Ferrari Mode: Nightly 2026-01-15 + Cranelift + Wild + Sccache"
  '';
}

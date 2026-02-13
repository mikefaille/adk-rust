{ pkgs, ... }:

{
  packages = [
    pkgs.git
    pkgs.glib
    pkgs.pkg-config
    pkgs.libva
    pkgs.libva.dev
    pkgs.alsa-lib
  ];

  languages.rust.enable = true;
  languages.rust.channel = "stable";

  # Optional: Enable pre-commit hooks
  pre-commit.hooks.rustfmt.enable = true;
  pre-commit.hooks.clippy.enable = true;
}

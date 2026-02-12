{ pkgs, ... }:

{
  packages = [
    pkgs.git
    pkgs.glib
    pkgs.pkg-config
    pkgs.openssl
  ];

  languages.rust.enable = true;
  languages.rust.channel = "stable";

  # Optional: Enable pre-commit hooks
  pre-commit.hooks.rustfmt.enable = true;
  pre-commit.hooks.clippy.enable = true;
}

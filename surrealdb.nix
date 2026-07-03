{ pkgs, lib, config, ... }:
let
  version = "3.1.5";
  src =
    if pkgs.stdenv.isDarwin then
      pkgs.fetchurl {
        url = "https://github.com/surrealdb/surrealdb/releases/download/v${version}/surreal-v${version}.darwin-arm64.tgz";
        sha256 = "476152bf16b974e13c9f8b6c78b8f91f605c7421cf7b221067399180fcb9394a";
      }
    else
      pkgs.fetchurl {
        url = "https://github.com/surrealdb/surrealdb/releases/download/v${version}/surreal-v${version}.linux-amd64.tgz";
        sha256 = "f7d515203ba0010bde3fc6a5706ce7327d356aca293fbba8424d442f5dcb5002";
      };
  surrealdb = pkgs.stdenv.mkDerivation {
    pname = "surrealdb";
    inherit version src;

    nativeBuildInputs = lib.optionals pkgs.stdenv.isLinux [ pkgs.autoPatchelfHook ];
    buildInputs = lib.optionals pkgs.stdenv.isLinux [ pkgs.stdenv.cc.cc.lib ];

    dontBuild = true;
    dontConfigure = true;

    sourceRoot = ".";

    installPhase = ''
      mkdir -p $out/bin
      cp surreal $out/bin/
      chmod +x $out/bin/surreal
    '';
  };

  surrealdb-wrapper = pkgs.writeShellScriptBin "surrealdb" ''
    exec "${config.devenv.root}/scripts/surrealdb" "$@"
  '';
in
{
  packages = [ surrealdb surrealdb-wrapper ];

  processes.surrealdb.exec = "surreal start --user root --pass root --allow-all --bind 0.0.0.0:8001 surrealkv://.devenv/state/surrealdb/zenith.db < /dev/null";

  env = {
    SURREAL_USER = "root";
    SURREAL_PASS = "root";
    SURREAL_SURREALKV_BLOCK_CACHE_CAPACITY = "1073741824";
  };
}

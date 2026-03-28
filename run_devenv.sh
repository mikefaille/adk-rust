#!/bin/bash
export PATH=$HOME/.nix-profile/bin:/nix/var/nix/profiles/default/bin:$PATH
source /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh
nix profile install "github:cachix/devenv/latest#devenv" --extra-experimental-features 'nix-command flakes'

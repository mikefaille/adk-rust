#!/bin/bash
curl --proto '=https' --tlsv1.2 -sSf -L https://install.determinate.systems/nix | sh -s -- install linux \
  --extra-conf "sandbox = false" \
  --init none \
  --no-confirm
. /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh
nix profile install "github:cachix/devenv/latest"

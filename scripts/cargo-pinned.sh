#!/usr/bin/env bash
# Run cargo on the rust-toolchain.toml-pinned toolchain.
#
# `cargo` on PATH is not necessarily rustup-managed: a shadowing cargo (e.g.
# from nix) silently ignores rust-toolchain.toml, so quality gates would run
# on the wrong compiler and fail on lints the pinned toolchain never emits.
# When rustup is present and has the pinned toolchain installed, route through
# it; otherwise fall back to plain cargo (devenv shells and CI already provide
# the right toolchain directly, and error if it is missing).
#
# Usage: scripts/cargo-pinned.sh <cargo args...>
#
# This prepends the toolchain's bin dir rather than `rustup run`: cargo
# resolves subcommands such as cargo-clippy from PATH, so routing only the
# cargo binary would pair a 1.95 cargo with a shadowing 1.98 clippy driver.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CHANNEL="$(sed -n 's/^channel *= *"//p' "$ROOT/rust-toolchain.toml" 2>/dev/null | cut -d'"' -f1 | head -1)"

if [[ -n "$CHANNEL" ]] && command -v rustup &>/dev/null; then
  if rustup toolchain list 2>/dev/null | grep -q "^${CHANNEL}"; then
    TOOLCHAIN="$(rustup toolchain list 2>/dev/null | grep -o "^${CHANNEL}[^ ]*" | head -1)"
    TOOLCHAIN_BIN="$(dirname "$(rustup which --toolchain "$TOOLCHAIN" cargo 2>/dev/null)")"
    if [[ -n "$TOOLCHAIN_BIN" && -d "$TOOLCHAIN_BIN" ]]; then
      export PATH="$TOOLCHAIN_BIN:$PATH"
    fi
  fi
fi
exec cargo "$@"

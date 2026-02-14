#!/usr/bin/env bash
#
# setup.sh: Idempotent Development Environment Setup
#
# This script prepares the development environment by installing Nix, devenv,
# and direnv with nix-direnv for optimal performance. It is designed to be
# run in environments where the user may not have sudo privileges.
#
# The script performs the following steps:
# 1. Installs Nix if it's not already present.
# 2. Configures Nix with flakes and cachix support.
# 3. Installs a specific version of devenv.
# 4. Installs direnv and the performance-critical nix-direnv using Nix.
# 5. Configures shell integration for direnv.
# 6. Creates a .envrc file for devenv integration.
# 7. Reloads the shell environment to make new commands available.

set -euo pipefail

# --- Constants ---
DEVENV_VERSION="v1.11.2"

# --- Helper Functions ---

# A consistent logging format.
# Usage: log "My message"
log() {
    echo "--- $1 ---"
}

# Checks if a command exists.
# Usage: command_exists "nix"
command_exists() {
    command -v "$1" &> /dev/null
}

# --- Installation Functions ---

# Installs Nix in single-user mode if it's not already installed.
install_nix() {
    log "Checking Nix installation"
    if command_exists "nix"; then
        echo "Nix is already installed. Skipping."
        return
    fi

    local nix_profile_script="$HOME/.nix-profile/etc/profile.d/nix.sh"
    if [ -f "$nix_profile_script" ]; then
        # shellcheck source=/dev/null
        . "$nix_profile_script"
    fi

    if command_exists "nix"; then
        echo "Nix is already installed. Skipping."
        return
    fi

    log "Removing old devenv versions to prevent conflicts..."
    nix profile remove devenv &>/dev/null || true # Ignore errors if not installed

    log "Installing Nix (single-user mode)"
    export NIX_CONFIG="filter-syscalls = false"
    sh <(curl --proto '=https' --tlsv1.2 -L https://nixos.org/nix/install) --no-daemon --yes
    log "Nix installation complete"
}

# Activates the Nix environment so the `nix` command is available.
activate_nix() {
    log "Activating Nix environment"
    local nix_profile_script="$HOME/.nix-profile/etc/profile.d/nix.sh"
    if [ -f "$nix_profile_script" ]; then
        # shellcheck source=/dev/null
        . "$nix_profile_script"
    fi
    nix --version
}

# Configures Nix with flakes support and cachix binary cache
configure_nix() {
    log "Configuring Nix with flakes and cachix support"

    # Create Nix config directory
    local nix_config_dir="$HOME/.config/nix"
    mkdir -p "$nix_config_dir"

    # Create/update nix.conf with required experimental features
    local nix_conf="$nix_config_dir/nix.conf"
    cat > "$nix_conf" << 'EOFC'
# Voice Gateway Nix Configuration
# Enable experimental features required for devenv
experimental-features = nix-command flakes

# Workaround for container environments (fixes seccomp BPF error)
filter-syscalls = false

# Add binary caches for faster builds (custom cache first for priority)
substituters = https://cache.nixos.org/ https://devenv.cachix.org https://mikefaille.cachix.org
trusted-public-keys = cache.nixos.org-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY= devenv.cachix.org-1:w1cLUi8dv3hnoSPGAuibQv+f9TZLr6cv/Hm9XgU50cw=

# Performance optimizations
max-jobs = auto
cores = 0
builders-use-substitutes = true

# Keep build outputs for debugging
keep-outputs = true
keep-derivations = true
EOFC

    echo "Created $nix_conf with flakes support and custom cache"

    # Verify flakes are now enabled
    if nix flake --help &>/dev/null; then
        echo "✅ Nix flakes are now enabled"
    else
        echo "⚠️  Warning: Flakes may not be fully enabled yet - may need shell restart"
    fi
}

# Installs the target version of devenv if not already installed.
install_devenv() {
    log "Checking devenv installation"
    if command_exists "devenv"; then

        local current_version
        current_version=$(devenv --version 2>/dev/null | head -n1 || echo "unknown")
        if [[ "$current_version" == *"$DEVENV_VERSION"* ]] || [[ "$current_version" == *"1.11.2"* ]]; then
            echo "devenv custom version is already installed. Skipping."
            return
        fi

    fi

    log "Removing old devenv versions to prevent conflicts..."
    nix profile remove devenv &>/dev/null || true # Ignore errors if not installed


    log "Installing custom devenv $DEVENV_VERSION"

    if ! command_exists "cachix"; then
        log "Installing cachix..."
        nix profile install --accept-flake-config nixpkgs#cachix
    fi

    # Use custom cache first, then fallback to official devenv cache
    log "Configuring binary caches..."
    cachix use mikefaille
    cachix use devenv


    # Install custom devenv from official repo
    log "Installing devenv..."
    nix profile install --accept-flake-config "github:cachix/devenv/$DEVENV_VERSION"


    devenv --version
    log "Custom devenv installation complete"
}

# Installs direnv and nix-direnv using the Nix profile for better integration.
install_direnv() {
    log "Installing direnv and nix-direnv"

    if command_exists "direnv"; then
        echo "direnv is already installed. Skipping direnv installation to avoid conflicts."
        echo "Ensuring nix-direnv is available..."
        # Try to install nix-direnv separately, but don't fail if it conflicts or exists
        nix profile install --accept-flake-config nixpkgs#nix-direnv || echo "Warning: nix-direnv installation had issues or is already present - continuing."
    else
        # Installing both together ensures compatibility and enables caching.
        nix profile install --accept-flake-config nixpkgs#direnv nixpkgs#nix-direnv
    fi

    log "direnv and nix-direnv installation steps completed"
}

# Configures direnv to integrate with the shell and use nix-direnv.
configure_direnv_integration() {
    log "Configuring direnv shell integration"

    # Configure direnv to use the nix-direnv integration for caching.
    local direnv_config_dir="$HOME/.config/direnv"
    mkdir -p "$direnv_config_dir"

    # Prefer local nix-direnv if available; otherwise use remote fallback
    local nix_direnv_rc_path="$HOME/.nix-profile/share/nix-direnv/direnvrc"
    if [ -f "$nix_direnv_rc_path" ]; then
        echo "source \"$nix_direnv_rc_path\"" > "$direnv_config_dir/direnvrc"
        echo "Configured nix-direnv integration in $direnv_config_dir/direnvrc (local)"
    else
        echo "⚠️  Warning: nix-direnv not found locally; using remote fallback"
        echo "source_url \"https://raw.githubusercontent.com/cachix/devenv/82c0147677e510b247d8b9165c54f73d32dfd899/direnvrc\" \"sha256-7u4iDd1nZpxL4tCzmPG0dQgC5V+/44Ba+tHkPob1v2k=\"" > "$direnv_config_dir/direnvrc"
        echo "Configured remote direnvrc fallback in $direnv_config_dir/direnvrc"
    fi

    # Add the direnv hook to the user's shell configuration.
    # This is idempotent; grep prevents adding duplicate lines.
    local direnv_hook='eval "$(direnv hook bash)"'
    local bashrc_file="$HOME/.bashrc"
    local zshrc_file="$HOME/.zshrc"
    local fish_config_file="$HOME/.config/fish/config.fish"

    # Detect current shell and add hook to appropriate config
    case "$SHELL" in
        */bash)
            if ! grep -qF -- "$direnv_hook" "$bashrc_file" 2>/dev/null; then
                echo "Adding direnv hook to $bashrc_file"
                echo -e "\n# direnv shell integration\n$direnv_hook" >> "$bashrc_file"
            else
                echo "direnv hook already in $bashrc_file. Skipping."
            fi
            ;;
        */zsh)
            if ! grep -qF -- "eval \"\$(direnv hook zsh)\"" "$zshrc_file" 2>/dev/null; then
                echo "Adding direnv hook to $zshrc_file"
                echo -e "\n# direnv shell integration\neval \"\$(direnv hook zsh)\"" >> "$zshrc_file"
            else
                echo "direnv hook already in $zshrc_file. Skipping."
            fi
            ;;
        */fish)
            if ! grep -qF -- "direnv hook fish" "$fish_config_file" 2>/dev/null; then
                echo "Adding direnv hook to $fish_config_file"
                mkdir -p "$(dirname "$fish_config_file")"
                echo -e "\n# direnv shell integration\ndirenv hook fish | source" >> "$fish_config_file"
            else
                echo "direnv hook already in $fish_config_file. Skipping."
            fi
            ;;
        *)
            echo "⚠️  Warning: Unsupported shell '$SHELL'. Add direnv hook manually."
            ;;
    esac
}

# Creates the .envrc file to automatically load the devenv environment.
create_envrc() {
    log "Creating .envrc file for devenv"
    local envrc_file=".envrc"
    if [ ! -f "$envrc_file" ]; then
        echo "use devenv" > "$envrc_file"
        echo ".envrc created."
    else
        # Ensure the content is correct, even if the file exists
        if ! grep -q -E "^\s*use devenv\s*$" "$envrc_file"; then
            echo "Updating existing .envrc with 'use devenv'"
            echo "use devenv" > "$envrc_file"
        else
            echo ".envrc already configured. Skipping."
        fi
    fi
}

# --- Main Execution ---

main() {
    log "Starting Development Environment Setup (Custom devenv build)"

    install_nix
    activate_nix
    configure_nix
    install_devenv
    install_direnv
    configure_direnv_integration
    create_envrc

    # Reload the Nix profile to make newly installed commands available
    log "Reloading Nix profile to update PATH"
    activate_nix

    # Source the profile script directly to update the current shell's PATH
    # This makes the new devenv version immediately available.
    # shellcheck source=/dev/null
    . "$HOME/.nix-profile/etc/profile.d/nix.sh"

    log "Setup Complete"
    echo -e "\n--- IMPORTANT ---"
    echo "The setup script has updated your environment with custom devenv build."
    echo "To activate the development environment, run:"
    echo "  direnv allow ."
    echo ""
    echo "After running 'direnv allow .', the environment will load automatically"
    echo "whenever you enter this directory."
    echo ""
    echo "You can verify the setup by running:"
    echo "  devenv shell"
    echo "  devenv --version"
}

main "$@"

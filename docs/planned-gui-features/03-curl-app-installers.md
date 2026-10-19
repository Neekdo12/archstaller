# Planned Feature: Curl-based Application Installers

## Overview
A declarative configuration mechanism allowing users to specify post-installation application scripts fetched via curl or git (e.g. Oh My Zsh, Starship, Rustup, Homebrew, NVM).

## Key Capabilities
- **Declarative Script Definitions**: Specify script URLs, expected checksums (SHA-256), environment variables, execution phases (pre-service or post-service), and executing user (root or unprivileged account).
- **Execution Sandboxing & Verification**: Scripts are downloaded into RAM, checksum-verified, and executed inside the target system chroot with explicit timeout boundaries.
- **GUI Catalog**: Built-in templates for popular command-line utilities, dotfile managers, and development toolchains selectable with a single toggle.

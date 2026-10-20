# Planned Feature: Curl-Based Application Installers

## Overview
This specification defines the declarative integration of curl-based application and development environment installers (e.g., Oh My Zsh, Rustup, Starship, NVM, SDKMAN, Volta) into the Archstaller GUI (Packages tab).

## Context & Motivation
Modern developer workflows commonly use curl-pipe-to-shell or direct remote installation scripts for tools not packaged in official Arch repositories or requiring user-local version management. Currently, users must manually configure complex post-install shell scripts or first-boot services to install these utilities. A declarative GUI catalogue simplifies developer environment bootstrapping.

## Specification

### 1. GUI Interface (Packages Tab)
- **Application Installer Catalogue**:
  - Searchable list of well-known community installers:
    - *Shell Environments*: Oh My Zsh, Fisher (Fish), Prezto, Starship prompt.
    - *Programming Language Toolchains*: Rustup (`sh.rustup.rs`), NVM, Volta, Pyenv, SDKMAN, Golang bootstrap.
    - *Developer Tools*: Homebrew (Linuxbrew), Nix package manager (single-user or multi-user).
  - Each catalogue item displays:
    - Tool name, icon/category badge, description, upstream installer URL.
    - Scope toggle: System-wide (root `/usr/local`) vs. Per-User (`$HOME/.local/bin` or `$HOME/.cargo`).
    - Pinned cryptographic digest (SHA-256) of the upstream install script with a "Fetch & Verify" action.
- **Custom Script Adder**:
  - URL entry field with automatic SHA-256 prefetching and checksum verification.
  - Option to pass arguments to the installer (e.g. `--unattended`, `--yes`).

### 2. Execution & Security Sandboxing
- Executed during the post-install firstboot phase (`archstaller-firstboot.target`):
  - When assigned to a user: executed as the unprivileged user with appropriate `$HOME`, `$USER`, and environment variables.
  - Scripts are downloaded over TLS, matched against the declared SHA-256 checksum, and written to a temporary execution wrapper.
  - Failures in curl installers are logged and warned, without halting the core operating system boot.

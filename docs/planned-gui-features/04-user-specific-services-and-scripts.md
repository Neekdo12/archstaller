# Planned Feature: User-Specific Services and Per-User Scripts

## Overview
Extends system service management to unprivileged user sessions via systemd user units (`systemctl --user`) and per-user environment initialization.

## Key Capabilities
- **Per-User Systemd Units**: Allows configuring custom service units located in `~/.config/systemd/user/` enabled automatically on user login via `systemctl --user enable`.
- **Linger Enablement**: Automatically toggles `loginctl enable-linger <user>` when persistent background services (e.g. syncthing, tmux, matrix-daemon) are specified for an account.
- **Per-User Login Scripts**: Declarative placement and execution of shell hooks (`~/.profile`, `~/.bashrc`, `~/.zshrc`, `~/.config/fish/config.fish`).

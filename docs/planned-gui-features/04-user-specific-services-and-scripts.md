# Planned Feature: User-Specific Services and Startup Scripts

## Overview
This specification describes the management of user-level systemd units (`systemctl --user`), per-user autostart configurations, and login environment scripts configured directly under individual user accounts in the Archstaller GUI (Users tab).

## Context & Motivation
Archstaller currently separates system-wide services and root scripts from user account definitions. User-level daemons (e.g. user audio servers, user ssh-agent, rclone mount services, syncthing, custom notification daemons, or Wayland compositors autostarts) are often tied to specific user profiles. Integrating user services and scripts into user configuration streamlines desktop workflow setup.

## Specification

### 1. GUI Interface (Users Tab)
- Inside each User account card:
  - **User Systemd Units**:
    - List of enabled user units (`~/.config/systemd/user/*.service`, `*.timer`).
    - Ability to add, edit inline, or import custom user units.
    - Checkbox: *Enable user lingering* (`loginctl enable-linger <user>`) allowing user background services to run without an active interactive login.
  - **Autostart & Login Scripts**:
    - Table of desktop autostart entries (`~/.config/autostart/*.desktop`).
    - Login environment script editor (appended to `~/.bash_profile`, `~/.zprofile`, or `~/.config/fish/config.fish`).
    - Per-user umask and environment variable dictionary.

### 2. Backend & Installer Engine Integration
- Target files written during first-boot installation into `$HOME`:
  - Automatic creation of `~/.config/systemd/user/` with correct UID/GID ownership and permissions (`0755` for directories, `0644` for unit files).
  - Calling `loginctl enable-linger <username>` if lingering is enabled.
  - Integration with user archive extraction and dotfiles linking to avoid file collision.

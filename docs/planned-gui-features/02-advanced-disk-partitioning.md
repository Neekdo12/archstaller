# Planned Feature: Advanced Multi-Distro Partitioning & Dual-Boot

## Overview
This specification details future graphical disk partitioning capabilities within the Archstaller GUI (Disk tab). It expands the current single-disk wipe model to support multi-distro coexistence, dual-booting with Windows/Linux, custom partition sizing, and fine-grained filesystem mount configurations.

## Context & Motivation
Archstaller currently assumes full disk ownership: it partitions the selected disk with an ESP (`/boot`) and a single root partition (`/`) formatted as ext4, btrfs, xfs, or f2fs. To serve as a versatile everyday installer, Archstaller needs the capability to install alongside existing operating systems, reuse existing ESP partitions, and allow manual partition schemes (e.g., dedicated `/home`, `/var/log`, or swap partitions).

## Specification

### 1. GUI Interface (Disk Tab)
- **Partitioning Mode Selector**:
  - `Erase Entire Disk`: The current automated scheme (ESP + Root partition).
  - `Install Alongside (Dual-Boot)`: Automatically detect free unpartitioned space on selected drives or offer to shrink an existing partition (NTFS, ext4, btrfs).
  - `Manual Partition Table Editor`:
    - Visual bar chart representing drive layout and partitions.
    - Partition table actions: Add Partition, Delete, Resize/Move, Format/Keep existing data.
    - Mount point assignment: Dropdown (`/`, `/boot`, `/home`, `/var`, `swap`, etc.) and custom paths.
    - Partition flags: EFI System Partition (ESP), Bootable, BIOS Boot partition (for legacy BIOS GPT setups).
    - Filesystem format selector: `ext4`, `btrfs`, `xfs`, `f2fs`, `swap`, or `Keep unformatted / existing`.

### 2. Btrfs Advanced Configuration
- Visual subvolume manager allowing custom subvolume naming and mount paths:
  - `@` -> `/`
  - `@home` -> `/home`
  - `@snapshots` -> `/.snapshots`
  - `@var_log` -> `/var/log`
- Compression algorithm selector: `zstd` (levels 1-15), `lzo`, `zlib`, or `none`.
- Subvolume snapshot policies (Snapper / Timeshift compatibility presets).

### 3. Backend & Installer Engine Integration
- Requires backend partition manipulation in `crates/gpt`, `crates/ext4w`, and `kernel/src/install.rs`:
  - Non-destructive partition creation without overwriting foreign partition table entries.
  - Integration with OS prober in bootloader setup (generating dual-boot GRUB/systemd-boot/Limine configuration entries).
  - Safe UUID preservation for existing filesystems.

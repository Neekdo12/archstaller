# Planned Feature: Advanced Disk Partitioning & Multi-Distro Dual Boot

## Overview
An interactive and declarative partitioning system extending the installer beyond the current auto-largest single-disk wipe model.

## Key Capabilities
- **Dual-Boot & Multi-Boot Coexistence**: Detects existing GPT/MBR partition tables (including existing Windows EFI and Linux installations), allowing installation into unallocated disk space without overwriting other OS partitions.
- **Custom Partition Layouts**: Enables defining custom partition sizes and types for ESP, `/boot`, swap, root, and dedicated sub-volumes.
- **Advanced Btrfs Subvolume Layouts**: UI and config support for standard nested or flat btrfs subvolumes (`@`, `@home`, `@snapshots`, `@var_log`, `@cache`) with individual mount options (compression like `zstd:1`, `noatime`).
- **Interactive Partition Editor in GUI**: Visual disk bar displaying existing partitions, sector alignment, mount points, and format toggles.

# Planned Feature: Seamless Ventoy Integration & Media Flashing

## Overview
This specification details enhanced Ventoy support within the Archstaller GUI header bar and media flashing workflows, providing seamless ISO copying to existing Ventoy USB drives and standalone Ventoy media preparation.

## Context & Motivation
Ventoy is the preferred deployment tool for many Linux sysadmins because a single USB flash drive can store multiple ISOs without wiping the drive. While Archstaller currently detects mounted Ventoy partitions when building an ISO, users desire a first-class flashing action that can:
1. Target Ventoy alongside raw block USB, optical media (CD/DVD), and floppy images.
2. Direct ISO file copying with verified SHA-256 integrity into designated Ventoy subdirectories (e.g. `/archstaller/`).
3. Offer an option to format a blank USB drive into a fresh Ventoy drive directly from Archstaller.

## Specification

### 1. GUI Interface (Media Target Selector)
- **Target Category Tabs**:
  - `Ventoy USB Drive`:
    - Discovers all connected USB storage devices containing a Ventoy label (`ventoy` exFAT/NTFS/ext4 partition + `VTOYEFI` ESP partition).
    - Displays drive capacity, available free space, and existing ISO files on the drive.
    - Path selector within Ventoy partition (e.g. `/` or `/isos/archstaller.iso`).
    - Action button: *Copy ISO to Ventoy* with live transfer speed and read-back SHA-256 verification.
  - `Raw Physical Media`:
    - USB Flash drive raw block write (`dd` equivalent via UDisks2).
    - Optical Disc writing via `xorriso` / `cdrecord`.
    - Raspberry Pi SD card / USB boot device (`kernel8.img` FAT32).
    - Floppy diskette image (`archstaller-floppy.img`).
  - `Prepare New Ventoy Drive`:
    - Optional utility action invoking `ventoy -i` / `ventoy2disk` to initialize a raw disk with the latest Ventoy payload before copying Archstaller onto it.

### 2. Backend & UDisks2 Integration
- Polkit / UDisks2 integration for non-root desktop environments.
- Direct sync and fsync after ISO copy, followed by an in-memory SHA-256 recalculation against the written block device or filesystem file.
- Automatic drive ejection/power-off option upon completion.

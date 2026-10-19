# Planned Feature: Enhanced Ventoy Flashing & Partition Integration

## Overview
Enhanced integration for Ventoy USB drives in both GUI and CLI build pipelines, avoiding full destructive disk formatting when a Ventoy filesystem is present.

## Key Capabilities
- **Automatic Ventoy Partition Discovery**: Detects standard Ventoy partition layouts (`Ventoy` exFAT/NTFS volume and `VTOYEFI` partition) via UDisks2 or blkid.
- **Non-Destructive File Deployment**: Copies `archstaller.iso` directly into the Ventoy data partition root or sub-directory with progress tracking and SHA-256 verification.
- **Ventoy Auto-Install Integration**: Generates matching `ventoy.json` or unattended boot configurations if requested.

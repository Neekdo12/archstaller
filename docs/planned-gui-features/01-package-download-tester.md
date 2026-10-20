# Planned Feature: In-RAM Package Download Tester

## Overview
The In-RAM Package Download Tester provides an interactive, non-destructive test suite within the Archstaller GUI (Build & drivers tab). It allows users to verify network connectivity, mirror throughput, pacman database synchronization, and package verification in RAM without partitioning or writing to any physical disk.

## Context & Motivation
Currently, testing the installation environment requires building and running the static `tester` preset (`configs/tester.toml`), which executes in QEMU or on bare metal with `dry_run = true`. Users cannot interactively customize what gets tested, benchmark mirror download speeds from their specific network, or verify that package signatures and download integrity succeed before committing to a full installation.

## Specification

### 1. GUI Interface (Build & Drivers Tab)
- **Interactive Test Configuration**:
  - Checkbox: *Verify DNS resolution and ping latency* to selected mirror hosts.
  - Checkbox: *Synchronize pacman databases* (`core.db`, `extra.db`) into an in-memory tmpfs.
  - Checkbox: *Sample Package Download*: Download a configurable set of packages (e.g. `linux`, `glibc`, `systemd`) directly into RAM (`/tmp` or RAM ring buffer).
  - Checkbox: *Integrity & Signature Check*: Validate downloaded package signatures against the Arch Linux master keyring.
  - Spinbutton: *Simulated Memory Budget*: Limit maximum RAM consumption allocated to testing (default: 512 MiB).
- **Execution & Feedback**:
  - "Run Diagnostics" button launching an asynchronous test task.
  - Progress bar displaying throughput (MiB/s), latency (ms), and byte count.
  - Test results log showing per-mirror response time and package validation status.

### 2. Backend & Installer Engine Integration
- Requires backend runner support in `kernel` and `hostcfg`:
  - Ephemeral RAM filesystem mount for package staging.
  - Mirror benchmarking routine recording HTTP/HTTPS connection establishment and transfer rates.
  - Dry-run validation mode reporting issues back via structured status messages.

### 3. Safety Guarantees
- Disks remain completely unmounted and write-protected (`dry_run` kernel enforcement).
- All downloaded test artifacts are discarded immediately upon test completion or cancellation.

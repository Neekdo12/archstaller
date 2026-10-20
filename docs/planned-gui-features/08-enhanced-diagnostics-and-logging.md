# Planned Feature: Enhanced Diagnostics & Kernel Ring Buffer Logging

## Overview
This specification details enhanced diagnostic tooling, kernel ring buffer log analysis, and interactive error remediation within the Archstaller GUI.

## Context & Motivation
Archstaller's installer kernel logs progress, network negotiation, disk partitioning, and errors into a 128 KiB in-RAM circular ring buffer (`kernel/src/logbuf.rs`). On failure, the kernel attempts to write this log to a Ventoy drive (`archstaller-log.txt`). For host-side users configuring complex installations or troubleshooting failed installs, the GUI should provide an interactive diagnostics viewer that imports log files or ring buffers and translates technical error messages into clear remediation steps.

## Specification

### 1. GUI Interface (Build & Drivers / System Tab / Problem Bar)
- **Log Level & Tracing Controls** (Build & drivers tab):
  - Checkboxes for kernel trace facilities:
    - Trace Network packets (ARP, DHCP, DNS, TCP/IP frame headers).
    - Trace USB & xHCI transaction packets.
    - Trace Storage & NVMe command queues.
    - Trace Hardware polling & idle PIT timers.
  - Runtime log verbosity slider: Error, Warn, Info, Debug, Trace.
- **Log Analyzer & Ring Buffer Inspector**:
  - "Import Installer Log" button to load `archstaller-log.txt` from a USB drive or file.
  - Interactive filterable log viewer with timestamp columns, log levels, and search filter.
  - "Analyze Failures" assistant:
    - Automatically parses common installer errors (e.g., DNS timeout, DHCP lease exhaustion, disk serial mismatch, missing kernel microcode, pacman mirror 404).
    - Recommends actionable configuration fixes directly within the Archstaller GUI (e.g., "Mirror timed out -> Click here to switch primary mirror to geo.mirror.pkgbuild.com").

### 2. Backend & Installer Engine Integration
- Standardized error codes across `kernel`, `bootinfo`, and `hostcfg`.
- Support for structured JSON or key-value event streams from the installer engine when running in test harness mode.

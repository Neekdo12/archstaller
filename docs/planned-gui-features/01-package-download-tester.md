# Planned Feature: In-RAM Package Download Tester

## Overview
A diagnostic and verification tool running directly in RAM before an installation begins. It performs a lightweight dry-run download of a designated subset of target packages from the configured mirrors.

## Key Capabilities
- **Mirror Latency & Throughput Benchmark**: Measures download speed, round-trip times, and TLS handshake latency across all configured mirrors in `[install.mirrors]`.
- **Integrity & Signature Verification**: Verifies downloaded archive signatures against the embedded Arch Linux keyring using `crates/pgp-lite`.
- **Pre-flight Network Health Check**: Detects broken mirrors, corrupted upstream databases, or captive portals before writing to persistent storage.
- **Configurable Test Profiles**: Allows testing minimal packages (e.g., `linux-firmware` chunk or `core/pacman`) without executing a full multi-gigabyte download.

# Planned Feature: GUI Wi-Fi Network Configuration

## Overview
This specification defines the GUI interface for wireless network discovery, SSID selection, security profile configuration (WPA2/WPA3-Personal, Enterprise, open networks), and credential provisioning for the installer and installed target.

## Context & Motivation
Currently, Wi-Fi networking is out of scope for the bare-metal kernel installer (which relies on polling Ethernet drivers and USB tethering as detailed in `OVERVIEW.md`). A comprehensive specification for future kernel Wi-Fi driver support is established in `plans-implement/wifi.md`. Once the backend driver architecture lands, the desktop GUI needs an intuitive visual configuration tool for managing wireless credentials.

## Specification

### 1. GUI Interface (Build & Drivers / System Tab)
- **Wi-Fi Network Scanner**:
  - Scan button querying host wireless adapters via `nmcli` or `iwd` dbus interfaces.
  - Signal strength indicators (RSSI bars), SSID name, channel/band (2.4 GHz / 5 GHz / 6 GHz), and security standard (Open, WPA2-PSK, WPA3-SAE, 802.1X EAP).
- **Network Profile Builder**:
  - SSID entry (with hidden network toggle).
  - Passphrase entry with visibility toggle and strength assessment.
  - Security selector: None, WPA-PSK, WPA2-Personal, WPA3-SAE, WPA-Enterprise.
  - Static IP / DHCP configuration (IP address, Gateway, DNS servers).
- **Target Installation Provisioning**:
  - Export profile to NetworkManager (`/etc/NetworkManager/system-connections/*.nmconnection` with `0600` permissions).
  - Export profile to iwd (`/var/lib/iwd/*.psk` with `0600` permissions).
  - Checkbox: *Embed Wi-Fi credentials into installer ISO* (allowing installer kernel to connect on boot once drivers exist).

### 2. Security & Credential Storage
- Wi-Fi passphrases are treated as sensitive secrets.
- Saved in configuration files with strict file permissions (`0600`).
- Option to generate pre-shared keys (`wpa_passphrase`) so cleartext passwords are not stored in plaintext configuration files.

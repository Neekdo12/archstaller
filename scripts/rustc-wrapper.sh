#!/bin/sh
set -eu
compiler=$1
shift
if [ -n "${CARGO_PKG_NAME:-}" ] && [ -n "${CARGO_CRATE_NAME:-}" ]; then
    set -- "$@" --cfg "archstaler_package=\"$CARGO_PKG_NAME\"" --cfg "archstaler_target=\"$CARGO_CRATE_NAME\"" --check-cfg 'cfg(archstaler_package, values("xtask", "aurbuild", "config", "pkg", "bootinfo", "drivers", "hal", "ext4w", "hostcfg", "initrd", "pgp-lite", "usbnet", "usb", "kernel", "disk", "imobiledevice", "net", "tiny-init", "archstaller-gui", "loadcore", "boot-uefi", "boot-bios-s1", "boot-bios"))' --check-cfg 'cfg(archstaler_target, values(any()))'
fi
exec "$compiler" "$@"

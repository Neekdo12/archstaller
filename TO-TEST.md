# TO-TEST.md

Work that is written but has not been compiled or run. Go through a section, tick what passed, and write what failed
under it. When a section passes completely, update the status line in its spec doc (named in the heading) and in
`OVERVIEW.md`, then delete the section. Delete this file when it is empty.

Everything here needs Linux with the nightly toolchain from `rust-toolchain.toml`.

## 1. Raw USB flashing (`plans-implement/raw-usb-flash.md`)

Code: `gui/src/flash.rs`, the flash dialog at the end of `gui/src/pages/iso.rs`, the new `Phase` variants and
`Report` fields in `gui/src/media.rs`. It has never been compiled.

### Build and unit tests

- [ ] `cargo build -p archstaller-gui` compiles. The riskiest parts are the gio D-Bus calls in `UDisks`
      (`call_sync`, `call_with_unix_fd_list_sync`, `UnixFDList::get`), checked against the gio 0.22.10 source
      but never built.
- [ ] `Cargo.lock` is still valid: `libc` was added to the `archstaller-gui` entry by hand. `cargo build --locked`
      should not want to change it.
- [ ] `cargo test -p archstaller-gui` passes, in particular the `flash::tests` (lsblk fixture, eligibility rules,
      identity check, writer against a temp file, cancel, read-back mismatch, a device that changes mid-way is
      never written).

### By hand, with a stick you can lose

Take a USB stick whose contents do not matter, and unplug every other USB drive, especially a Ventoy stick.

- [ ] With a Ventoy stick plugged in, the "Flash ISO to USB" button does **not** appear and the Ventoy copy works
      as before.
- [ ] Without Ventoy, the ISO page shows "No Ventoy drive found" and the button. It is greyed out until an ISO is
      built or picked.
- [ ] The dialog lists the stick, with **nothing selected**. Internal disks are not offered ("N internal disk(s)
      are never offered").
- [ ] A stick mounted by the desktop (under `/run/media/...`) is offered, and its mount is shown.
- [ ] Ineligible drives show a reason. Try one or more of: a stick that is too small for the ISO, a write-protected
      SD card, a stick with an open LUKS volume, a stick that is mounted somewhere outside `/run/media`, `/media`
      and `/mnt`.
- [ ] "Erase and flash" stays greyed out until the drive's name (`sdb`) is typed exactly. Selecting another drive
      clears what was typed.
- [ ] Flashing asks for your password (polkit), then the progress bar goes through unmounting, writing, flushing,
      verifying and powering off.
- [ ] The result says "Flashed and verified /dev/sdX (... bytes, sha256 ...)". Compare the hash with
      `sha256sum` of the ISO.
- [ ] Check the stick independently: `sudo cmp -n "$(stat -c %s the.iso)" the.iso /dev/sdX` prints nothing.
- [ ] The flashed stick boots the installer in UEFI mode and in BIOS (legacy/CSM) mode on a real machine. In QEMU:
      `qemu-system-x86_64 -m 2G -drive file=/dev/sdX,format=raw,if=virtio` (BIOS); add
      `-bios /usr/share/ovmf/x64/OVMF.4m.fd` for UEFI. Use a tester-style config so nothing gets installed.
- [ ] Cancel during writing: the result says the device is partially overwritten and not verified, and the app
      keeps working.
- [ ] Pull the stick out while it is being written: an error, no crash, and no other disk was touched.
- [ ] Press Cancel in the password prompt: "could not open ... Nothing was written".
- [ ] On a system without UDisks2 (or with the service stopped), the dialog says raw flashing is not possible and
      the folder copy still works.
- [ ] If powering off is refused (some card readers), the result still says "verified" and tells you to use safe
      removal.

Write down the desktop, UDisks2 version (`udisksctl status` or `busctl get-property org.freedesktop.UDisks2
/org/freedesktop/UDisks2/Manager org.freedesktop.UDisks2.Manager Version`), util-linux version, and the stick's
model in the spec doc once it passes.

## 2. Sway preset (`plans-implement/sway-preset.md`)

Code: `configs/sway.lua`, `configs/sway-config.sh`, and the new `scripts` field in `configs/common.lua`. The package
names were checked against the Arch databases of 2026-10-07; nothing was resolved, built or installed.

### Config checks

- [ ] `cargo test -p hostcfg` passes. It loads every config in `configs/` (`sway.lua` included) and checks
      that the preset is in the table in `docs/lua-config.md`.
- [ ] `cargo xtask build --config configs/sway.lua --out /tmp/sway.iso` embeds `sway-config.sh` without an error
      (local script paths are relative to the config file).
- [ ] `cargo xtask check-presets` resolves `sway` with no missing package and with `jack` coming from
      `pipewire-jack`. Write the real package count and download size into the preset tables in
      `docs/lua-config.md` and `README.md` (they now say "~1.1 GiB (estimate)").
- [ ] `cargo xtask presets` builds `target/isos/archstaller-sway.iso`.
- [ ] `bash -n configs/sway-config.sh` and, if installed, `shellcheck configs/sway-config.sh` are clean.

### Install in QEMU

- [ ] `cargo xtask e2e --config configs/sway.lua` passes (install, first boot, second boot to the login prompt).
- [ ] In the serial log of the first boot (under `target/e2e/`): `running script sway-config`, then
      `sway: wrote ~/.config/sway/config for passwd_is_passwd` and `script sway-config: done`.
- [ ] Boot the installed disk with graphics. Change the file name to match your e2e mode:
      `qemu-system-x86_64 -enable-kvm -m 4G -drive file=target/e2e/disk-bios-virtio-virtio.img,format=raw,if=virtio -device virtio-vga-gl -display gtk,gl=on -nic user,model=virtio-net-pci`
- [ ] ly lists a Sway session and logs in as `passwd_is_passwd` / `passwd`.
- [ ] Sway starts with waybar (icons show, so `otf-font-awesome` is enough), `Super+Return` opens foot,
      `Super+d` opens wofi, `Super+Shift+c` reloads without a second waybar.
- [ ] `notify-send hi` shows a mako notification. `Print` puts a screenshot in the clipboard (`wl-paste | file -`).
- [ ] `Super+Ctrl+l` locks with swaylock and unlocks with the password.
- [ ] `/etc/skel/.config/sway/config` exists, and a user added with `useradd -m` gets the config.
- [ ] Firefox starts, and its file dialog opens (xdg-desktop-portal-gtk).
- [ ] When graphics fail (boot QEMU with `-vga none -nographic`), the system still boots to a text console login.

### Real hardware

- [ ] Install on a machine with an integrated GPU: keyboard, sound (`wpctl status`, volume keys), brightness keys on
      a laptop, a second monitor, networking through NetworkManager.
- [ ] Try the commented-out swayidle block in `~/.config/sway/config`. If locking on idle and resuming work, enable
      it in `configs/sway-config.sh`.
- [ ] Log out cleanly (`Super+Shift+e`) back to ly.

## 3. Official package search: import without pacman (`docs/gui-app.md`)

Code: `pacman_names` and `OfficialState::poll` in `gui/src/official.rs`, `render_import` in
`gui/src/pages/packages.rs`. The search, the live total and the import on Arch were run in the GUI; this path was not.

- [ ] On a system without `pacman` (or with `pacman` hidden from `PATH`), "Import from this system" shows
      "Import failed: pacman could not be run (...)" and the package list is unchanged; nothing panics.
- [ ] With the package databases failing to load (no mirror), Import shows "the package databases are not loaded
      (...)" instead of adding names unchecked.


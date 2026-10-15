#!/bin/bash
# Runs once on the first boot of a system laid down by archstaller. All configuration comes from
# data files in /var/lib/archstaller; nothing from the config is interpolated into this script.
set -u
D=/var/lib/archstaller
say() { echo "archstaller-firstboot: $*" | tee /dev/console; }
fail() { say "FAILED: $*"; exit 1; }

say "initializing pacman keyring"
pacman-key --init || fail "pacman-key --init"
pacman-key --populate archlinux || fail "pacman-key --populate"

say "installing packages (runs scriptlets and hooks)"
mapfile -t PKGS < "$D/packages.list"
(cd /var/cache/pacman/pkg && pacman -U --noconfirm --overwrite '*' "${PKGS[@]}") || fail "pacman -U"
if [ -s "$D/deps.list" ]; then
    xargs -a "$D/deps.list" pacman -D --asdeps || say "warning: could not mark dependencies"
fi

say "applying system configuration"
cp -a "$D/overlay/." / || fail "overlay"
locale-gen || say "warning: locale-gen failed"

# The ly login manager: show the banner above the login box (a .dur file drawn as ly's background).
if [ -f /etc/ly/config.ini ] && [ -f /etc/ly/archstaller.dur ]; then
    sed -i -E 's|^animation = .*|animation = dur_file|; s|^dur_file_path = .*|dur_file_path = /etc/ly/archstaller.dur|; s|^dur_offset_alignment = .*|dur_offset_alignment = topcenter|; s|^dur_y_offset = .*|dur_y_offset = 2|' /etc/ly/config.ini ||
        say "warning: could not set up the ly banner"
fi

if [ -f "$D/root.hash" ]; then
    usermod -p "$(cat "$D/root.hash")" root || say "warning: root password"
fi
while IFS=: read -r name hash groups shell; do
    [ -n "$name" ] || continue
    if [ -n "$groups" ]; then
        for g in ${groups//,/ }; do getent group "$g" >/dev/null || groupadd "$g"; done
        useradd -m -s "$shell" -G "$groups" "$name" || fail "useradd $name"
    else
        useradd -m -s "$shell" "$name" || fail "useradd $name"
    fi
    usermod -p "$hash" "$name" || fail "password for $name"
    say "user $name created"
done < "$D/users.list"

# Files fetched by the installer (third-party configs); one copy per user, owned by that user.
if [ -s "$D/userfiles.list" ]; then
    while IFS=: read -r uname _rest; do
        [ -n "$uname" ] || continue
        home=$(getent passwd "$uname" | cut -d: -f6)
        grp=$(id -gn "$uname")
        while IFS=: read -r idx dest; do
            [ -n "$idx" ] || continue
            runuser -u "$uname" -- mkdir -p "$(dirname "$home/$dest")" &&
                install -o "$uname" -g "$grp" -m 0644 "$D/userfiles/$idx" "$home/$dest" &&
                say "user file ~/$dest installed for $uname" ||
                say "warning: could not install ~/$dest for $uname"
        done < "$D/userfiles.list"
    done < "$D/users.list"
fi

# Zip archives fetched by the installer, unpacked into each user's home as that user.
if [ -s "$D/userarchives.list" ]; then
    while IFS=: read -r uname _rest; do
        [ -n "$uname" ] || continue
        home=$(getent passwd "$uname" | cut -d: -f6)
        while read -r idx; do
            [ -n "$idx" ] || continue
            runuser -u "$uname" -- bsdtar -xf "$D/userarchives/$idx" -C "$home" &&
                say "user archive #$idx extracted into ~ for $uname" ||
                say "warning: could not extract user archive #$idx for $uname"
        done < "$D/userarchives.list"
    done < "$D/users.list"
fi

# AUR packages are built on the next boot by archstaller-aur.service, once the network is up; enabling it here
# does not wait for it, so a missing network never blocks this first boot.
if [ -s "$D/aur.list" ]; then
    systemctl enable archstaller-aur.service || say "warning: could not enable the AUR build service"
fi

# Custom systemd services: installed into /etc/systemd/system (root) or ~/.config/systemd/user (user).
if [ -s "$D/custom_services.list" ]; then
    say "installing custom systemd services"
    while IFS=: read -r idx uname en unit; do
        [ -n "$idx" ] || continue
        src="$D/custom_services/$idx"
        [ -f "$src" ] || continue
        if [ "$uname" = "root" ] || [ -z "$uname" ]; then
            install -m 0644 "$src" "/etc/systemd/system/$unit" || say "warning: could not install /etc/systemd/system/$unit"
            if [ "$en" = "1" ]; then
                echo "$unit" >> "$D/services.list"
            fi
            say "custom system service $unit installed"
        else
            home=$(getent passwd "$uname" | cut -d: -f6)
            grp=$(id -gn "$uname")
            if [ -n "$home" ] && [ -d "$home" ]; then
                user_unit_dir="$home/.config/systemd/user"
                runuser -u "$uname" -- mkdir -p "$user_unit_dir"
                install -o "$uname" -g "$grp" -m 0644 "$src" "$user_unit_dir/$unit" || say "warning: could not install $unit for $uname"
                if [ "$en" = "1" ]; then
                    targets=$(grep -E '^\s*WantedBy\s*=' "$user_unit_dir/$unit" | tail -n1 | cut -d= -f2)
                    for target in ${targets:-default.target}; do
                        target=$(echo "$target" | tr -d ' ')
                        [ -n "$target" ] || continue
                        wants_dir="$user_unit_dir/${target}.wants"
                        runuser -u "$uname" -- mkdir -p "$wants_dir"
                        ln -sf "../$unit" "$wants_dir/$unit"
                        chown -h "$uname:$grp" "$wants_dir/$unit"
                    done
                    say "custom user service $unit enabled for $uname"
                else
                    say "custom user service $unit installed for $uname (disabled)"
                fi
            else
                say "warning: user $uname not found for custom service $unit"
            fi
        fi
    done < "$D/custom_services.list"
fi

if [ -s "$D/services.list" ]; then
    xargs -a "$D/services.list" systemctl enable || say "warning: enabling some services failed"
    while read -r unit; do say "service $unit: $(systemctl is-enabled "$unit" 2>&1)"; done < "$D/services.list"
fi
# systemd-resolved only answers programs that read /etc/resolv.conf (most of them) when it points at its stub.
if grep -qx 'systemd-resolved.service' "$D/services.list" 2>/dev/null; then
    ln -sf ../run/systemd/resolve/stub-resolv.conf /etc/resolv.conf && say "/etc/resolv.conf points at systemd-resolved" ||
        say "warning: could not point /etc/resolv.conf at systemd-resolved"
fi

# Configured scripts (config `scripts`), in order, as root. Each runs from a file under $D/scripts; its
# arguments are plain words from scripts.list (validated: no spaces, quotes or shell syntax) passed as
# separate arguments. Output is bounded, a failure or timeout only warns.
if [ -s "$D/scripts.list" ]; then
    while IFS=: read -r idx sid sargs; do
        [ -n "$idx" ] || continue
        read -r -a sargv <<< "$sargs"
        say "running script $sid"
        # stdin is /dev/null: otherwise a script that reads input eats the rest of scripts.list.
        timeout 900 bash "$D/scripts/$idx.sh" "${sargv[@]}" < /dev/null 2>&1 | head -c 65536
        rc=${PIPESTATUS[0]}
        if [ "$rc" -eq 0 ]; then say "script $sid: done"; else say "warning: script $sid exited with status $rc"; fi
    done < "$D/scripts.list"
fi

# The journal is created when the file system is written (crates/ext4w), not here: adding one with
# `tune2fs -j` while the root is mounted leaves a /.journal file that only a later fsck turns into the
# journal inode, and a boot that mounts first fails with "failed to locate journal superblock".

# Leave out the "kms" hook (early graphics): it loads the GPU driver before the root file system is
# mounted, and a GPU that fails to initialise (missing firmware, a chip the driver cannot handle) then
# stops the boot with "Fatal error during GPU init" and no root. The GPU driver loads later instead.
if grep -q '^HOOKS=.* kms' /etc/mkinitcpio.conf; then
    sed -i '/^HOOKS=/ s/ kms\b//' /etc/mkinitcpio.conf && say "initramfs: early graphics (kms) disabled"
fi

say "building initramfs"
mkinitcpio -P || fail "mkinitcpio"

# The kernel to boot: `linux` if it is installed, else the first other kernel package (each installs a
# mkinitcpio preset named after itself, and /boot/vmlinuz-<name> + initramfs-<name>.img). The installer made
# the same choice for this first boot and put that kernel on the ESP as vmlinuz-linux.
KERNEL=linux
if [ ! -f /etc/mkinitcpio.d/linux.preset ]; then
    for p in /etc/mkinitcpio.d/*.preset; do
        [ -f "$p" ] && KERNEL=$(basename "$p" .preset) && break
    done
fi
[ -f "/boot/vmlinuz-$KERNEL" ] && [ -f "/boot/initramfs-$KERNEL.img" ] || fail "no /boot/vmlinuz-$KERNEL or initramfs-$KERNEL.img to boot"
say "boot kernel: $KERNEL"

say "installing GRUB"
ESP_DEV=$(findmnt -no SOURCE /boot)
# GRUB finds /boot by its file system UUID: searching for a file name can land on another disk's ESP (an
# older install, the installer stick) and boot that kernel with this system's command line.
ESP_UUID=$(findmnt -no UUID /boot)
if [ -n "$ESP_UUID" ]; then
    SEARCH="--fs-uuid --set=root $ESP_UUID"
else
    SEARCH="--file --set=root /vmlinuz-$KERNEL"
fi
DISK=$(lsblk -no PKNAME "$ESP_DEV")
mkdir -p /boot/grub
{
    echo "set timeout=3"
    echo "set default=0"
    echo
    echo "menuentry 'Arch Linux' {"
    echo "    search --no-floppy $SEARCH"
    echo "    linux /vmlinuz-$KERNEL $(cat "$D/cmdline")"
    echo "    initrd /initramfs-$KERNEL.img"
    echo "}"
} > /boot/grub/grub.cfg || fail "grub.cfg"
if [ -d /sys/firmware/efi ]; then
    grub-install --target=x86_64-efi --efi-directory=/boot --boot-directory=/boot --bootloader-id=GRUB --removable || fail "grub-install (efi)"
    PART=$(cat "/sys/class/block/${ESP_DEV##*/}/partition")
    efibootmgr --create --disk "/dev/$DISK" --part "$PART" --label "Arch Linux" --loader '\EFI\BOOT\BOOTX64.EFI' || say "warning: efibootmgr failed"
else
    grub-install --target=i386-pc --boot-directory=/boot "/dev/$DISK" || fail "grub-install (bios)"
fi
rm -rf /boot/limine
# What only the first boot needed: archstaller's loader config and initramfs, and the installer's copy of a
# kernel that is not `linux` (it was put on the ESP as vmlinuz-linux).
rm -f /boot/archstaller.cfg /boot/initramfs-archstaller.img
[ "$KERNEL" = linux ] || rm -f /boot/vmlinuz-linux

rm -f "$D/firstboot"
say "done"
systemctl --no-block reboot

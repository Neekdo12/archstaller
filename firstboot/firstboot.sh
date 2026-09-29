#!/bin/bash
# Runs once on the first boot of a system laid down by archstaler. All configuration comes from
# data files in /var/lib/archstaler; nothing from the config is interpolated into this script.
set -u
D=/var/lib/archstaler
say() { echo "archstaler-firstboot: $*" | tee /dev/console; }
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
done < "$D/users.list"

if [ -s "$D/services.list" ]; then
    xargs -a "$D/services.list" systemctl enable || say "warning: enabling some services failed"
fi

ROOT_DEV=$(findmnt -no SOURCE /)
tune2fs -j "$ROOT_DEV" || say "warning: could not add a journal"

say "building initramfs"
mkinitcpio -P || fail "mkinitcpio"

say "installing boot loader entries"
{
    echo "timeout: 3"
    echo
    echo "/Arch Linux"
    echo "    protocol: linux"
    echo "    path: boot():/vmlinuz-linux"
    echo "    cmdline: $(cat "$D/cmdline")"
    echo "    module_path: boot():/initramfs-linux.img"
} > /boot/limine/limine.conf || fail "limine.conf"
if [ -d /sys/firmware/efi ]; then
    ESP_DEV=$(findmnt -no SOURCE /boot)
    DISK=$(lsblk -no PKNAME "$ESP_DEV")
    PART=$(cat "/sys/class/block/${ESP_DEV##*/}/partition")
    efibootmgr --create --disk "/dev/$DISK" --part "$PART" --label "Arch Linux" --loader '\EFI\BOOT\BOOTX64.EFI' || say "warning: efibootmgr failed"
fi

rm -f "$D/firstboot"
say "done"
systemctl --no-block reboot

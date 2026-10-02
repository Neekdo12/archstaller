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

if [ -s "$D/services.list" ]; then
    xargs -a "$D/services.list" systemctl enable || say "warning: enabling some services failed"
    while read -r unit; do say "service $unit: $(systemctl is-enabled "$unit" 2>&1)"; done < "$D/services.list"
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

say "installing GRUB"
ESP_DEV=$(findmnt -no SOURCE /boot)
DISK=$(lsblk -no PKNAME "$ESP_DEV")
mkdir -p /boot/grub
{
    echo "set timeout=3"
    echo "set default=0"
    echo
    echo "menuentry 'Arch Linux' {"
    echo "    search --no-floppy --file --set=root /vmlinuz-linux"
    echo "    linux /vmlinuz-linux $(cat "$D/cmdline")"
    echo "    initrd /initramfs-linux.img"
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

rm -f "$D/firstboot"
say "done"
systemctl --no-block reboot

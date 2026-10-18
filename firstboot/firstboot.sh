#!/bin/bash
# Runs once on the first boot of a system laid down by archstaller. All configuration comes from
# data files in /var/lib/archstaller; nothing from the config is interpolated into this script.
set -u
D=/var/lib/archstaller
export ARCHSTALLER_DATA="$D"
export ARCHSTALLER_OVERLAY="$D/overlay"
say() { echo "archstaller-firstboot: $*" | tee /dev/console; }
fail() { say "FAILED: $*"; exit 1; }

run_scripts() {
    local target_phase="$1"
    [ -s "$D/scripts.list" ] || return 0
    while IFS=: read -r idx sid phase sargs; do
        [ -n "$idx" ] || continue
        if [ -z "$sargs" ] && [ "$phase" != "pre-service" ] && [ "$phase" != "post-service" ]; then
            sargs="$phase"
            phase="post-service"
        fi
        [ "$phase" = "$target_phase" ] || continue
        read -r -a sargv <<< "$sargs"
        say "running $phase script $sid"
        # stdin is /dev/null: otherwise a script that reads input eats the rest of scripts.list.
        timeout 900 bash "$D/scripts/$idx.sh" "${sargv[@]}" < /dev/null 2>&1 | head -c 65536
        rc=${PIPESTATUS[0]}
        if [ "$rc" -eq 0 ]; then say "script $sid: done"; else say "warning: script $sid exited with status $rc"; fi
    done < "$D/scripts.list"
}

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
if [ -f /etc/ly/archstaller.dur ] || [ -f "$D/overlay/etc/ly/archstaller.dur" ]; then
    mkdir -p /etc/ly "$D/overlay/etc/ly"
    [ -f /etc/ly/archstaller.dur ] || cp -a "$D/overlay/etc/ly/archstaller.dur" /etc/ly/archstaller.dur
    if [ -f /etc/ly/config.ini ]; then
        if grep -q '^animation =' /etc/ly/config.ini; then
            sed -i -E 's|^animation = .*|animation = dur_file|' /etc/ly/config.ini
        else
            echo "animation = dur_file" >> /etc/ly/config.ini
        fi
        if grep -q '^dur_file_path =' /etc/ly/config.ini; then
            sed -i -E 's|^dur_file_path = .*|dur_file_path = /etc/ly/archstaller.dur|' /etc/ly/config.ini
        else
            echo "dur_file_path = /etc/ly/archstaller.dur" >> /etc/ly/config.ini
        fi
        if grep -q '^dur_offset_alignment =' /etc/ly/config.ini; then
            sed -i -E 's|^dur_offset_alignment = .*|dur_offset_alignment = topcenter|' /etc/ly/config.ini
        else
            echo "dur_offset_alignment = topcenter" >> /etc/ly/config.ini
        fi
        if grep -q '^dur_y_offset =' /etc/ly/config.ini; then
            sed -i -E 's|^dur_y_offset = .*|dur_y_offset = 2|' /etc/ly/config.ini
        else
            echo "dur_y_offset = 2" >> /etc/ly/config.ini
        fi
    else
        cat <<'EOF' > /etc/ly/config.ini
animation = dur_file
dur_file_path = /etc/ly/archstaller.dur
dur_offset_alignment = topcenter
dur_y_offset = 2
EOF
    fi
    cp -a /etc/ly/config.ini "$D/overlay/etc/ly/config.ini" 2>/dev/null || true
    cp -a /etc/ly/archstaller.dur "$D/overlay/etc/ly/archstaller.dur" 2>/dev/null || true
fi

# greetd login manager: tuigreet with --issue
if [ -f "$D/overlay/etc/greetd/config.toml" ] || [ -f /etc/greetd/config.toml ]; then
    mkdir -p /etc/greetd "$D/overlay/etc/greetd"
    if [ ! -f /etc/greetd/config.toml ] && [ -f "$D/overlay/etc/greetd/config.toml" ]; then
        cp -a "$D/overlay/etc/greetd/config.toml" /etc/greetd/config.toml
    elif [ -f /etc/greetd/config.toml ]; then
        if ! grep -q -- '--issue' /etc/greetd/config.toml && grep -q 'tuigreet' /etc/greetd/config.toml; then
            sed -i 's|tuigreet|tuigreet --issue|' /etc/greetd/config.toml
        fi
        cp -a /etc/greetd/config.toml "$D/overlay/etc/greetd/config.toml" 2>/dev/null || true
    fi
fi

# sddm login manager: archstaller theme branding
if [ -f "$D/overlay/etc/sddm.conf.d/10-archstaller.conf" ] || [ -d /etc/sddm.conf.d ]; then
    mkdir -p /etc/sddm.conf.d "$D/overlay/etc/sddm.conf.d"
    if [ -f "$D/overlay/etc/sddm.conf.d/10-archstaller.conf" ] && [ ! -f /etc/sddm.conf.d/10-archstaller.conf ]; then
        cp -a "$D/overlay/etc/sddm.conf.d/10-archstaller.conf" /etc/sddm.conf.d/10-archstaller.conf
    fi
fi

# lightdm-gtk-greeter login manager: archstaller branding
if [ -f "$D/overlay/etc/lightdm/lightdm-gtk-greeter.conf" ] || [ -d /etc/lightdm ]; then
    mkdir -p /etc/lightdm "$D/overlay/etc/lightdm"
    if [ -f "$D/overlay/etc/lightdm/lightdm-gtk-greeter.conf" ] && [ ! -f /etc/lightdm/lightdm-gtk-greeter.conf ]; then
        cp -a "$D/overlay/etc/lightdm/lightdm-gtk-greeter.conf" /etc/lightdm/lightdm-gtk-greeter.conf
    elif [ -f /etc/lightdm/lightdm-gtk-greeter.conf ]; then
        if ! grep -q 'archstaller.svg' /etc/lightdm/lightdm-gtk-greeter.conf; then
            sed -i -E 's|^#?logo = .*|logo = /usr/share/pixmaps/archstaller.svg|' /etc/lightdm/lightdm-gtk-greeter.conf
            sed -i -E 's|^#?background = .*|background = #1a1b26|' /etc/lightdm/lightdm-gtk-greeter.conf
        fi
        cp -a /etc/lightdm/lightdm-gtk-greeter.conf "$D/overlay/etc/lightdm/lightdm-gtk-greeter.conf" 2>/dev/null || true
    fi
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

# User archives (.tar.zst, .tar.xz, .tar.gz, .zip):
# Downloaded, loaded from disk UUID, or unpacked from embedded image into each user's home.
if [ -s "$D/userarchives.list" ]; then
    mkdir -p "$D/userarchives"
    while IFS=: read -r idx kind spec path sha; do
        [ -n "$idx" ] || continue
        archive_file="$D/userarchives/$idx"
        
        # 1. Fetch or load if not already embedded
        if [ "$kind" = "url" ] && [ ! -s "$archive_file" ]; then
            url="$spec"
            say "downloading user archive #$idx from $url"
            # If network is not up yet, try bringing up NetworkManager or systemd-networkd
            if ! ping -c 1 -W 2 1.1.1.1 >/dev/null 2>&1 && ! ping -c 1 -W 2 8.8.8.8 >/dev/null 2>&1; then
                if grep -qx 'NetworkManager.service' "$D/services.list" 2>/dev/null || systemctl is-enabled NetworkManager >/dev/null 2>&1; then
                    systemctl start NetworkManager || true
                elif grep -qx 'systemd-networkd.service' "$D/services.list" 2>/dev/null || systemctl is-enabled systemd-networkd >/dev/null 2>&1; then
                    systemctl start systemd-networkd || true
                    systemctl start systemd-resolved || true
                fi
                for _ in {1..15}; do
                    ping -c 1 -W 1 1.1.1.1 >/dev/null 2>&1 && break
                    sleep 1
                done
            fi
            if curl -fSL --retry 3 --connect-timeout 10 "$url" -o "$archive_file.tmp"; then
                mv "$archive_file.tmp" "$archive_file"
                say "user archive #$idx downloaded successfully"
            else
                rm -f "$archive_file.tmp"
                say "warning: could not download user archive #$idx from $url"
                continue
            fi
        elif [ "$kind" = "uuid" ] && [ ! -s "$archive_file" ]; then
            uuid="$spec"
            subpath="${path:-home.tar.zst}"
            say "searching for archive '$subpath' on disk UUID $uuid"
            udevadm settle --timeout=5 2>/dev/null || true
            dev="/dev/disk/by-uuid/$uuid"
            if [ ! -b "$dev" ]; then
                dev=$(findfs "UUID=$uuid" 2>/dev/null || true)
            fi
            if [ -n "$dev" ] && [ -b "$dev" ]; then
                mkdir -p /mnt/archstaller-archive
                if mount -o ro "$dev" /mnt/archstaller-archive; then
                    src_file="/mnt/archstaller-archive/$subpath"
                    if [ -f "$src_file" ]; then
                        cp "$src_file" "$archive_file"
                        say "user archive #$idx loaded from disk UUID $uuid"
                    else
                        say "warning: file $subpath not found on disk UUID $uuid"
                    fi
                    umount /mnt/archstaller-archive || true
                else
                    say "warning: failed to mount disk UUID $uuid ($dev)"
                fi
                rmdir /mnt/archstaller-archive 2>/dev/null || true
            else
                say "warning: disk with UUID $uuid not found"
            fi
        elif [ "$kind" = "file" ] && [ ! -s "$archive_file" ]; then
            src_file="$spec"
            if [ -f "$src_file" ]; then
                cp "$src_file" "$archive_file"
            else
                say "warning: file $src_file not found"
            fi
        fi

        # 2. Check SHA-256 digest if configured
        if [ -n "$sha" ] && [ -s "$archive_file" ]; then
            calc_sha=$(sha256sum "$archive_file" | cut -d' ' -f1)
            if [ "$calc_sha" != "$sha" ]; then
                say "warning: user archive #$idx sha256 mismatch (got $calc_sha, expected $sha); skipping"
                rm -f "$archive_file"
                continue
            fi
        fi

        # 3. Extract into each user's home
        if [ -s "$archive_file" ]; then
            while IFS=: read -r uname _rest; do
                [ -n "$uname" ] || continue
                home=$(getent passwd "$uname" | cut -d: -f6)
                [ -n "$home" ] && [ -d "$home" ] || continue
                runuser -u "$uname" -- bsdtar -xf "$archive_file" -C "$home" &&
                    say "user archive #$idx extracted into ~ for $uname" ||
                    say "warning: could not extract user archive #$idx for $uname"
            done < "$D/users.list"
        fi
    done < "$D/userarchives.list"
fi

# GitHub Dotfiles & Stow: clone or download dotfiles repository into ~/dotfiles and link via stow.
if [ -s "$D/dotfiles.list" ]; then
    say "setting up user dotfiles and stow"
    while IFS=: read -r uname repo dir branch stow pkgs; do
        [ -n "$uname" ] || continue
        home=$(getent passwd "$uname" | cut -d: -f6)
        grp=$(id -gn "$uname")
        [ -n "$home" ] && [ -d "$home" ] || continue
        target_dir="$home/$dir"
        say "fetching dotfiles for $uname from $repo into ~/$dir"

        # Check internet connectivity
        if ! ping -c 1 -W 2 1.1.1.1 >/dev/null 2>&1 && ! ping -c 1 -W 2 8.8.8.8 >/dev/null 2>&1; then
            if systemctl is-enabled NetworkManager >/dev/null 2>&1; then
                systemctl start NetworkManager || true
                for _ in {1..15}; do
                    ping -c 1 -W 1 1.1.1.1 >/dev/null 2>&1 && break
                    sleep 1
                done
            fi
        fi

        cloned=0
        if command -v git >/dev/null 2>&1; then
            clone_cmd=(git clone --depth 1)
            [ -n "$branch" ] && clone_cmd+=(-b "$branch")
            clone_cmd+=("https://github.com/$repo.git" "$target_dir")
            if runuser -u "$uname" -- "${clone_cmd[@]}"; then
                cloned=1
            fi
        fi
        if [ "$cloned" -eq 0 ]; then
            runuser -u "$uname" -- mkdir -p "$target_dir"
            branch_name="${branch:-main}"
            tar_url="https://github.com/$repo/archive/refs/heads/${branch_name}.tar.gz"
            if curl -fSL --retry 3 --connect-timeout 10 "$tar_url" | runuser -u "$uname" -- bsdtar -xzf - --strip-components=1 -C "$target_dir"; then
                cloned=1
            fi
        fi

        if [ "$cloned" -eq 1 ]; then
            chown -R "$uname:$grp" "$target_dir" || true
            say "dotfiles successfully installed for $uname"
            if [ "$stow" = "1" ]; then
                if command -v stow >/dev/null 2>&1; then
                    say "stowing dotfiles for $uname in ~/$dir"
                    if [ -n "$pkgs" ]; then
                        for pkg in $pkgs; do
                            runuser -u "$uname" -- stow -d "$target_dir" -t "$home" "$pkg" || say "warning: stow package $pkg failed for $uname"
                        done
                    else
                        runuser -u "$uname" -- stow -d "$target_dir" -t "$home" . || say "warning: stow . failed for $uname"
                    fi
                    say "stow completed for $uname"
                else
                    say "warning: stow is not installed, skipping stow linking for $uname"
                fi
            fi
        else
            say "warning: could not download dotfiles from $repo for $uname"
        fi
    done < "$D/dotfiles.list"
fi

# AUR packages are built on the next boot by archstaller-aur.service, once the network is up; enabling it here
# does not wait for it, so a missing network never blocks this first boot.
if [ -s "$D/aur.list" ]; then
    systemctl enable archstaller-aur.service || say "warning: could not enable the AUR build service"
fi

# Pre-service scripts: runs before custom services and systemctl enable
run_scripts "pre-service"

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

# Post-service scripts (default phase)
run_scripts "post-service"

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

BOOTLOADER="grub"
if [ -f "$D/bootloader" ]; then
    BOOTLOADER="$(cat "$D/bootloader" | tr -d '[:space:]')"
fi
say "installing bootloader: $BOOTLOADER"
ESP_DEV=$(findmnt -no SOURCE /boot)
ESP_UUID=$(findmnt -no UUID /boot)
DISK=$(lsblk -no PKNAME "$ESP_DEV")

case "$BOOTLOADER" in
    systemd-boot)
        if [ ! -d /sys/firmware/efi ]; then
            fail "systemd-boot requires a UEFI system"
        fi
        bootctl install --esp-path=/boot || fail "bootctl install"
        mkdir -p /boot/loader/entries
        cat <<EOF > /boot/loader/loader.conf
default arch.conf
timeout 3
editor no
EOF
        cat <<EOF > /boot/loader/entries/arch.conf
title Arch Linux
linux /vmlinuz-$KERNEL
initrd /initramfs-$KERNEL.img
options $(cat "$D/cmdline")
EOF
        ;;
    limine)
        mkdir -p /boot/limine /boot/EFI/BOOT
        if [ -f /usr/share/limine/limine-bios.sys ]; then
            cp -a /usr/share/limine/limine-bios.sys /boot/limine/ 2>/dev/null || cp -a /usr/share/limine/limine-bios.sys /boot/
        fi
        cat <<EOF > /boot/limine.conf
timeout: 3
/Arch Linux
    protocol: linux
    kernel_path: boot():/vmlinuz-$KERNEL
    module_path: boot():/initramfs-$KERNEL.img
    cmdline: $(cat "$D/cmdline")
EOF
        if [ -d /sys/firmware/efi ]; then
            if [ -f /usr/share/limine/BOOTX64.EFI ]; then
                cp -a /usr/share/limine/BOOTX64.EFI /boot/EFI/BOOT/BOOTX64.EFI
            fi
            PART=$(cat "/sys/class/block/${ESP_DEV##*/}/partition")
            efibootmgr --create --disk "/dev/$DISK" --part "$PART" --label "Arch Linux" --loader '\EFI\BOOT\BOOTX64.EFI' || say "warning: efibootmgr failed"
        else
            limine bios-install "/dev/$DISK" || fail "limine bios-install"
        fi
        ;;
    *)
        # Default: GRUB
        if [ -n "$ESP_UUID" ]; then
            SEARCH="--fs-uuid --set=root $ESP_UUID"
        else
            SEARCH="--file --set=root /vmlinuz-$KERNEL"
        fi
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
        ;;
esac

if [ "$BOOTLOADER" != "limine" ]; then
    rm -rf /boot/limine
fi
# What only the first boot needed: archstaller's loader config and initramfs, and the installer's copy of a
# kernel that is not `linux` (it was put on the ESP as vmlinuz-linux).
rm -f /boot/archstaller.cfg /boot/initramfs-archstaller.img
[ "$KERNEL" = linux ] || rm -f /boot/vmlinuz-linux

rm -f "$D/firstboot"
say "done"
systemctl --no-block reboot

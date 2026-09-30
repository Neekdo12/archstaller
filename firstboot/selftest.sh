#!/bin/bash
# Post-install self test for the "tester" preset. Runs on the boot of the installed system,
# checks what the installer and the first boot should have produced, prints a report to the
# console and reboots after WAIT seconds. Read-only: it changes nothing on the system.
set -u
D=/var/lib/archstaler
WAIT=${SELFTEST_WAIT:-60}
PASS=0; FAIL=0; WARN=0

out() { echo "$*" | tee /dev/console; }
ok()   { PASS=$((PASS + 1)); out "[ OK ] $*"; }
bad()  { FAIL=$((FAIL + 1)); out "[FAIL] $*"; }
warn() { WARN=$((WARN + 1)); out "[WARN] $*"; }
check() { # check "description" command...
    local d=$1; shift
    if "$@" >/dev/null 2>&1; then ok "$d"; else bad "$d"; fi
}

out ""
out "=============== archstaler self test ==============="
out "kernel: $(uname -r)   boot: $([ -d /sys/firmware/efi ] && echo UEFI || echo BIOS)   up: $(cut -d' ' -f1 /proc/uptime)s"

# Give the network a moment; DHCP may still be running.
for _ in $(seq 1 20); do
    ip -4 route show default 2>/dev/null | grep -q . && break
    sleep 1
done

out "--- first boot ---"
check "first boot finished (no $D/firstboot marker)" test ! -e "$D/firstboot"
check "boot loader config exists (/boot/limine/limine.conf)" test -s /boot/limine/limine.conf
check "initramfs built (/boot/initramfs-linux.img)" test -s /boot/initramfs-linux.img
check "kernel image present (/boot/vmlinuz-linux)" test -s /boot/vmlinuz-linux
if grep -q archstaler-firstboot /proc/cmdline; then bad "booted the first-boot entry again"; else ok "booted the final entry"; fi

out "--- filesystems ---"
check "root is ext4" test "$(findmnt -no FSTYPE /)" = ext4
check "root is mounted rw" bash -c 'findmnt -no OPTIONS / | grep -qw rw'
check "/boot is vfat" test "$(findmnt -no FSTYPE /boot)" = vfat
if tune2fs -l "$(findmnt -no SOURCE /)" 2>/dev/null | grep -q 'features:.*has_journal'; then ok "root has an ext4 journal"; else bad "root has no ext4 journal"; fi
out "  $(df -h --output=target,size,used,avail / /boot | tail -n +2 | tr -s ' ' | paste -sd'|')"

out "--- configuration ---"
if [ -r "$D/cmdline" ]; then
    EXP_HOST=$(cat /etc/hostname 2>/dev/null)
    [ "$(hostname)" = "$EXP_HOST" ] && ok "hostname $(hostname)" || bad "hostname $(hostname) != /etc/hostname $EXP_HOST"
fi
check "timezone link (/etc/localtime)" test -e /etc/localtime
check "locale generated ($(sed -n 's/^LANG=//p' /etc/locale.conf 2>/dev/null))" bash -c 'locale -a | grep -qi "$(sed -n "s/^LANG=//p" /etc/locale.conf | sed "s/UTF-8/utf8/")"'
check "vconsole keymap configured" grep -q '^KEYMAP=' /etc/vconsole.conf
check "mirrorlist written" grep -q '^Server' /etc/pacman.d/mirrorlist
check "sudoers wheel rule" test -s /etc/sudoers.d/10-wheel

out "--- users ---"
while IFS=: read -r name _hash groups shell; do
    [ -n "$name" ] || continue
    if getent passwd "$name" >/dev/null; then
        ok "user $name exists (shell $(getent passwd "$name" | cut -d: -f7), home $(getent passwd "$name" | cut -d: -f6))"
        [ -d "$(getent passwd "$name" | cut -d: -f6)" ] || bad "home of $name missing"
        for g in ${groups//,/ }; do id -nG "$name" | tr ' ' '\n' | grep -qx "$g" || bad "$name not in group $g"; done
        pw=$(getent shadow "$name" | cut -d: -f2)
        case "$pw" in '$6$'*) ok "$name has a SHA-512 password hash";; *) bad "$name has no usable password hash";; esac
    else
        bad "user $name missing"
    fi
done < "$D/users.list"
if [ -f "$D/root.hash" ]; then :; else
    case "$(getent shadow root | cut -d: -f2)" in '!'*|'*'*) ok "root is locked";; *) warn "root is not locked";; esac
fi

out "--- packages ---"
WANT=$(grep -c . "$D/packages.list" 2>/dev/null || echo 0)
HAVE=$(pacman -Q 2>/dev/null | wc -l)
[ "$HAVE" -ge "$WANT" ] && ok "$HAVE packages installed (installer downloaded $WANT)" || bad "only $HAVE packages installed, installer downloaded $WANT"
missing=0
while read -r f; do
    n=${f%-*-*-*}
    pacman -Qq "$n" >/dev/null 2>&1 || { missing=$((missing + 1)); out "  missing: $n"; }
done < "$D/packages.list"
[ "$missing" -eq 0 ] && ok "every downloaded package is registered in the pacman DB" || bad "$missing package(s) not registered"
qk=$(pacman -Qk 2>&1 | grep -v ' 0 missing files' | head -n 5)
[ -z "$qk" ] && ok "pacman -Qk clean" || { warn "pacman -Qk reports missing files:"; out "$qk"; }
check "pacman keyring initialised" test -d /etc/pacman.d/gnupg

out "--- services ---"
while read -r unit; do
    [ -n "$unit" ] || continue
    en=$(systemctl is-enabled "$unit" 2>&1); ac=$(systemctl is-active "$unit" 2>&1)
    if [ "$en" = enabled ] && [ "$ac" = active ]; then ok "$unit enabled + active"
    elif [ "$en" = enabled ]; then bad "$unit enabled but $ac"
    else bad "$unit is $en"; fi
done < "$D/services.list"
state=$(systemctl is-system-running 2>&1)
case "$state" in running) ok "systemd state: running";; *) bad "systemd state: $state";; esac
failed=$(systemctl --failed --no-legend --plain 2>/dev/null | awk '{print $1}')
if [ -z "$failed" ]; then ok "no failed units"; else bad "failed units:"; echo "$failed" | sed 's/^/  /' | tee /dev/console; fi

out "--- network ---"
ifs=$(ip -o link show | awk -F': ' '$2 != "lo" {print $2}' | paste -sd' ')
[ -n "$ifs" ] && ok "interfaces: $ifs" || bad "no network interface"
addr=$(ip -4 -o addr show scope global | awk '{print $2 " " $4}' | paste -sd',')
[ -n "$addr" ] && ok "IPv4: $addr" || bad "no global IPv4 address"
check "default route" bash -c 'ip -4 route show default | grep -q .'
check "DNS resolves geo.mirror.pkgbuild.com" getent hosts geo.mirror.pkgbuild.com
if command -v curl >/dev/null; then
    check "HTTPS reaches the Arch mirror" curl -fsS --max-time 15 -o /dev/null -I https://geo.mirror.pkgbuild.com/core/os/x86_64/core.db
else
    warn "curl not installed, HTTPS check skipped"
fi
[ "$(timedatectl show -p NTPSynchronized --value 2>/dev/null)" = yes ] && ok "clock synchronised" || warn "clock not (yet) synchronised"

out "--- user files ---"
if [ -s "$D/userfiles.list" ] || [ -s "$D/userarchives.list" ]; then
    while IFS=: read -r name _; do
        [ -n "$name" ] || continue
        home=$(getent passwd "$name" | cut -d: -f6)
        while IFS=: read -r _i dest; do
            [ -n "$dest" ] || continue
            check "~/$dest for $name" test -s "$home/$dest"
        done < "$D/userfiles.list"
        [ -s "$D/userarchives.list" ] && { [ -n "$(ls -A "$home" 2>/dev/null | head -n 1)" ] && ok "home of $name is populated" || bad "home of $name is empty"; }
    done < "$D/users.list"
else
    out "  (none configured)"
fi

out "--- kernel and hardware ---"
n=$(journalctl -k -p err --no-pager -q 2>/dev/null | wc -l)
[ "$n" -eq 0 ] && ok "no kernel errors" || { warn "$n kernel error line(s):"; journalctl -k -p err --no-pager -q | tail -n 5 | sed 's/^/  /' | tee /dev/console; }
out "  cpu: $(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2 | xargs)   mem: $(awk '/MemTotal/ {printf "%d MiB", $2/1024}' /proc/meminfo)"
out "  disks: $(lsblk -dno NAME,SIZE,MODEL 2>/dev/null | grep -v '^zram' | paste -sd';')"

out "===================================================="
if [ "$FAIL" -eq 0 ]; then out "RESULT: PASS   ($PASS ok, $WARN warnings, 0 failed)"; else out "RESULT: FAIL   ($PASS ok, $WARN warnings, $FAIL failed)"; fi
out "archstaler-selftest: done"
for ((i = WAIT; i > 0; i--)); do
    if [ $((i % 10)) -eq 0 ] || [ "$i" -le 5 ]; then out "rebooting in ${i}s..."; fi
    sleep 1
done
systemctl --no-block reboot

#!/bin/bash
# Builds and installs the AUR packages of the config on the installed system. Runs from
# archstaler-aur.service after the first boot, once the network is up. Everything comes from data files in
# /var/lib/archstaler; nothing from the config is interpolated into this script.
#
# Per package (aur.list, in build order): fetch the pinned commit of the AUR repository, check that the tree
# is the one the user reviewed (a SHA-256 over every file), build it as an unprivileged user, install the
# result with pacman -U. A failure only warns. A package that cannot be fetched (no network) stays pending
# and is tried again on the next boot, at most five times in all; a tree that does not match its digest is
# refused for good.
set -u
D=${ARCHSTALER_STATE:-/var/lib/archstaler}
LIST=$D/aur.list
PENDING=$D/aur.pending
TRIES=$D/aur.tries
LOG=${ARCHSTALER_LOG:-/var/log/archstaler-aur.log}
BU=archstaler-build
BH=$D/aur-build
MAXTRIES=5
# The two overrides below exist for the tests (a local directory of repositories); the service sets neither.
AUR=${ARCHSTALER_AUR_BASE:-https://aur.archlinux.org}
CACHE=${ARCHSTALER_CACHE:-/var/cache/pacman/pkg}
say() { echo "archstaler-aur: $*" | tee -a "$LOG"; }
as_build() { runuser -u "$BU" -- env HOME="$BH" LC_ALL=C.UTF-8 "$@"; }

# SHA-256 over the sorted lines "<sha256 of file>  <path>" of every file but .git (plans-implement/aur.md, "Pinning").
tree_digest() {
    (cd "$1" && find . -path ./.git -prune -o -type f -printf '%P\n' | LC_ALL=C sort |
        while IFS= read -r f; do printf '%s  %s\n' "$(sha256sum < "$f" | cut -d' ' -f1)" "$f"; done) | sha256sum | cut -d' ' -f1
}

finish() {
    systemctl disable archstaler-aur.service >/dev/null 2>&1
    rm -f "$PENDING" "$PENDING.new" "$TRIES"
    rm -rf "$BH"
    userdel "$BU" >/dev/null 2>&1
}

[ -s "$LIST" ] || { finish; exit 0; }
n=$(cat "$TRIES" 2>/dev/null || echo 0)
n=$((n + 1))
echo "$n" > "$TRIES"
if [ "$n" -gt "$MAXTRIES" ]; then
    say "giving up after $MAXTRIES attempts, see $LOG"
    finish
    exit 0
fi
[ -f "$PENDING" ] || cp "$LIST" "$PENDING"
[ -f "$LOG" ] && [ "$(stat -c %s "$LOG")" -gt 8388608 ] && tail -c 1048576 "$LOG" > "$LOG.tmp" && mv "$LOG.tmp" "$LOG"

if [[ $AUR == https://* ]]; then
    for _ in $(seq 1 30); do getent hosts "${AUR#https://}" >/dev/null 2>&1 && break; sleep 2; done
    getent hosts "${AUR#https://}" >/dev/null 2>&1 || { say "warning: no network, will try again on the next boot (attempt $n of $MAXTRIES)"; exit 1; }
fi

id "$BU" >/dev/null 2>&1 || useradd -r -m -d "$BH" -s /usr/bin/nologin "$BU" || { say "FAILED: cannot create the build user"; exit 1; }
install -d -o "$BU" -g "$BU" -m 700 "$BH"

: > "$PENDING.new"
while IFS= read -r line; do
    [ -n "$line" ] || continue
    IFS=: read -r name pkgbase commit sha vcs asdep services <<< "$line"
    dir=$BH/$pkgbase
    say "package $name: fetching $pkgbase at $commit"
    rm -rf "$dir"
    if ! as_build git clone --quiet "$AUR/$pkgbase.git" "$dir" >>"$LOG" 2>&1; then
        say "warning: could not fetch $pkgbase, will try again on the next boot"
        echo "$line" >> "$PENDING.new"
        continue
    fi
    if ! as_build git -C "$dir" checkout --quiet "$commit" >>"$LOG" 2>&1 || [ "$(as_build git -C "$dir" rev-parse HEAD)" != "$commit" ]; then
        say "REFUSED $name: the commit $commit is not in the repository"
        continue
    fi
    got=$(tree_digest "$dir")
    if [ "$got" != "$sha" ]; then
        say "REFUSED $name: the recipe does not match what was reviewed (digest $got, expected $sha)"
        continue
    fi
    [ "$vcs" = 1 ] && say "note: $name builds from a VCS source that the commit does not pin"
    say "package $name: building (this can take a long time)"
    if ! as_build bash -c 'cd "$1" && exec timeout 7200 makepkg --noconfirm --clean --nocolor' _ "$dir" >>"$LOG" 2>&1; then
        say "warning: building $name failed, see $LOG"
        continue
    fi
    file=
    while IFS= read -r f; do
        base=${f##*/}
        [[ $base == "$name"-* ]] || continue
        rest=${base#"$name"-}
        rest=${rest%%.pkg.tar*}
        [ "$(tr -cd - <<< "$rest" | wc -c)" -eq 2 ] && file=$f && break
    done < <(as_build bash -c 'cd "$1" && makepkg --packagelist' _ "$dir" 2>>"$LOG")
    if [ -z "$file" ] || [ ! -f "$file" ]; then
        say "warning: no package file for $name was produced"
        continue
    fi
    opts=(--noconfirm --needed)
    [ "$asdep" = 1 ] && opts+=(--asdeps)
    if ! pacman -U "${opts[@]}" "$file" >>"$LOG" 2>&1; then
        say "warning: pacman refused $name, see $LOG"
        continue
    fi
    cp -f "$file" "$CACHE/" 2>/dev/null
    say "package $name: installed"
    for u in ${services//,/ }; do
        systemctl enable "$u" >>"$LOG" 2>&1 && say "service $u enabled" || say "warning: could not enable $u"
    done
done < "$PENDING"

if [ -s "$PENDING.new" ]; then
    mv "$PENDING.new" "$PENDING"
    say "some packages are still pending (attempt $n of $MAXTRIES)"
    exit 1
fi
say "done"
finish

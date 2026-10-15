#!/bin/sh
# Build and install archstaller on Arch Linux without the AUR:
#   curl -fsSL https://raw.githubusercontent.com/Neekdo12/archstaller/master/packaging/install.sh | sh
# or, from a checkout: packaging/install.sh
#
# Run from a checkout, it installs from the local tree instead of a fresh clone. It fingerprints the
# working directory (every tracked file's content, so uncommitted edits count, plus untracked, non-ignored
# files) and, when nothing changed since the last install, does nothing. When something changed, it
# rebuilds in a persistent cache directory: cargo recompiles only the crates whose sources changed, and
# makepkg repackages the result. Without a checkout it builds the AUR package from git as before.
set -eu

if ! command -v pacman >/dev/null 2>&1; then
    echo "install.sh needs Arch Linux (pacman)" >&2
    exit 1
fi
if [ "$(id -u)" -eq 0 ]; then
    echo "run as a normal user: makepkg refuses root (sudo is used where needed)" >&2
    exit 1
fi

sudo pacman -S --needed --noconfirm base-devel git

# --- local checkout ---------------------------------------------------------
dir=$(cd "$(dirname "$0")" 2>/dev/null && pwd || true)
# Only a real git checkout can be fingerprinted and rebuilt incrementally; a plain tarball of the
# sources (no .git) falls through to the remote build below.
if [ -n "$dir" ] && [ -f "$dir/aur/archstaller-git/PKGBUILD.local.in" ] \
    && command -v git >/dev/null 2>&1 && [ -d "$dir/../.git" ]; then
    root=$(cd "$dir/.." && pwd)
    tmpl="$dir/aur/archstaller-git/PKGBUILD.local.in"

    # Fingerprint the whole working tree: every tracked file's content (git ls-files lists them whether or
    # not they differ from HEAD) plus untracked, non-ignored files, and the files that decide how the tree
    # is built. A committed, an edited and a new source file all change the hash.
    fingerprint() {
        cd "$root"
        {
            git ls-files -z --cached --others --exclude-standard \
                | sort -z \
                | xargs -0 -r sha256sum
            sha256sum rust-toolchain.toml .cargo/config.toml Cargo.lock Cargo.toml \
                packaging/archstaller packaging/org.archstaller.Gui.desktop \
                packaging/org.archstaller.Gui.svg 2>/dev/null || true
        } | sha256sum | cut -d' ' -f1
    }

    cache=${XDG_CACHE_HOME:-$HOME/.cache}/archstaller/install-build
    want=$(fingerprint)
    stamp="$cache/.installed-fingerprint"
    if [ -f "$stamp" ] && [ "$(cat "$stamp")" = "$want" ] && command -v archstaller-gui >/dev/null 2>&1; then
        echo "install.sh: local tree unchanged since the last install; nothing to do."
        exit 0
    fi

    # pkgver from the local git, the same shape the AUR PKGBUILD produces. No pipefail: the describe
    # output is captured first, so a tagless tree falls back without depending on the shell.
    pkgver=$(
        cd "$root" || exit 1
        desc=$(git describe --long --tags --abbrev=7 2>/dev/null) || desc=
        if [ -n "$desc" ]; then
            printf '%s' "$desc" | sed 's/^v//;s/\([^-]*-g\)/r\1/;s/-/./g'
        else
            printf 'r%s.%s' "$(git rev-list --count HEAD)" "$(git rev-parse --short=7 HEAD)"
        fi
    )

    # A persistent source tree: copy only the files git tracks (plus untracked, non-ignored ones), so
    # target/, .git/, ISOs and archives stay out. Files removed from the tree are pruned, so cargo sees
    # exactly the working tree on every run.
    src="$cache/src"
    mkdir -p "$src"
    list=$(cd "$root" && git ls-files --cached --others --exclude-standard)
    if [ -f "$src/.filelist" ]; then
        while IFS= read -r f; do
            [ -n "$f" ] || continue
            printf '%s\n' "$list" | grep -qxF "$f" || rm -f "$src/$f"
        done < "$src/.filelist"
    fi
    printf '%s\n' "$list" > "$src/.filelist"
    printf '%s\n' "$list" | while IFS= read -r f; do
        [ -n "$f" ] || continue
        mkdir -p "$src/$(dirname "$f")"
        cp -p "$root/$f" "$src/$f"
    done

    work="$cache/makepkg"
    pkgdest="$cache/pkg"
    target="$cache/target"
    mkdir -p "$work" "$pkgdest" "$target"

    # Fill the template with the paths for this run; makepkg then builds the synced tree in place.
    sed -e "s|@PKGVER@|$pkgver|g" -e "s|@SRC@|$src|g" -e "s|@TARGET@|$target|g" \
        "$tmpl" > "$work/PKGBUILD"

    echo "install.sh: building the local tree (incremental) ..."
    # makepkg clears its own $srcdir/$pkgdir; the real tree and the cargo target live in the cache and
    # survive, so repeated runs recompile only what changed.
    ( cd "$work" && PKGDEST="$pkgdest" BUILDDIR="$cache/makepkg-build" makepkg -si --noconfirm )

    mkdir -p "$(dirname "$stamp")"
    printf '%s' "$want" > "$stamp"
    echo "Installed from the local tree. Run 'archstaller-gui' or 'archstaller'."
    exit 0
fi

# --- fallback: no checkout, build the AUR package from git ------------------
work=$(mktemp -d)
curl -fsSL -o "$work/PKGBUILD" \
    https://raw.githubusercontent.com/Neekdo12/archstaller/master/packaging/aur/archstaller-git/PKGBUILD

cd "$work"
# -s installs the dependencies, -i installs the package
makepkg -si --noconfirm
cd /
rm -rf "$work"

echo "Installed. Run 'archstaller-gui' or 'archstaller'."

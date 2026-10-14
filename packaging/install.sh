#!/bin/sh
# Build and install archstaller on Arch Linux without the AUR:
#   curl -fsSL https://raw.githubusercontent.com/Neekdo12/archstaller/master/packaging/install.sh | sh
# or, from a checkout: packaging/install.sh
# It installs the build tools, builds packaging/aur/archstaller-git with makepkg and installs the package.
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

dir=$(cd "$(dirname "$0")" 2>/dev/null && pwd || true)
if [ -n "$dir" ] && [ -f "$dir/aur/archstaller-git/PKGBUILD" ]; then
    src="$dir/aur/archstaller-git"
    work=$(mktemp -d)
    cp "$src/PKGBUILD" "$work/"
else
    work=$(mktemp -d)
    curl -fsSL -o "$work/PKGBUILD" \
        https://raw.githubusercontent.com/Neekdo12/archstaller/master/packaging/aur/archstaller-git/PKGBUILD
fi

cd "$work"
# -s installs the dependencies, -i installs the package
makepkg -si --noconfirm
cd /
rm -rf "$work"

echo "Installed. Run 'archstaller-gui' or 'archstaller'."

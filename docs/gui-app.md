# The desktop app

`archstaller-gui` is a GTK 4 app for Linux (Windows and macOS are not supported). It needs the `gtk4` and
`gtksourceview5` libraries at runtime (Arch: `pacman -S gtk4 gtksourceview5`) and their development files plus
`pkg-config` to build it. Run it from inside the checkout, or set `ARCHSTALLER_ROOT`; it finds the presets and
builds the ISO through `cargo xtask`.

With a prebuilt binary:

```sh
cargo build --profile gui -p archstaller-gui
target/gui/archstaller-gui
```

Rebuild after every code change.

## Features

- **Pages:** System, Disk, Mirrors & packages (official package search, live total, import, a resolve preview and the AUR group), Users, Services & kernel,
  Build & drivers, Scripts, Lua source, Build ISO. A red dot in the sidebar marks a page with a validation
  problem; the bar at the bottom shows the first one with a button that goes there. The checks are the command
  line's.
- **Menus and keys:** File menu, From preset, Tools. `Ctrl+N` new, `Ctrl+O` open, `Ctrl+S` save,
  `Ctrl+Shift+S` save as, `Ctrl+K` (or `/` outside a text field) opens the command launcher, which lists the
  same actions plus every page and preset.
- **Presets:** open as an unsaved config and say so when they erase the largest disk.
- **Lua source:** shows the generated Lua with syntax highlighting. "Edit source" switches to source mode:
  the form pages are unavailable, you edit the text (undo, search with `Ctrl+F`, `Ctrl+Space` suggestions for
  field names, driver IDs, built-in scripts, profile names, package and service names), and the diagnostics line
  shows the loader's answer, with "Go to line" when the message names one. "Return to the forms" works when
  the text is well formed and passes the loader's shape and type checks. An incomplete config, such as one with
  no disk chosen, comes back and the problem bar says what is missing. A file the GUI did not write opens in
  source mode and is replaced by form output only after you confirm. Suggestions are hints: the loader decides
  what a config means.
- **Build ISO:** builds from the saved file (an unsaved config is saved first, or built from a temporary copy
  when the target is a Ventoy drive), shows the stages and the log, can cancel, and copies the finished ISO
  onto a mounted Ventoy volume as a file with a SHA-256 read-back check. Without a Ventoy drive it can also copy
  into a folder of any mounted volume, or, only when you press **Flash ISO to USB (erases device)** (always offered, so a blank stick works; it saves the config, builds the ISO, flashes it and deletes the ISO afterwards, unless you picked an ISO by hand), write the
  ISO over a whole USB stick: the dialog lists only removable USB disks that hold neither the running system nor
  the ISO, preselects none, and starts after you type the disk's name (`sdb`). The stick is unmounted and opened
  through UDisks2 (your desktop asks for your password), written from its first byte, read back and compared by
  SHA-256, then powered off. This erases everything on the stick and is Linux only; it needs UDisks2 2.7.3 or
  newer and util-linux 2.37 or newer. Not yet tested on a real stick.
- **Theme and size:** the window is dark by default, whatever the system theme says
  (it sets `GTK_THEME=Adwaita:dark` at start unless you set `GTK_THEME` yourself; `ARCHSTALLER_THEME=system` follows the system, `ARCHSTALLER_THEME=light` forces light). Its default size is
  90% of the screen at most. A window narrower than 760 px hides the sidebar (the header's toggle brings it
  back), and long labels wrap, so it works in a tiling-window-manager tile or a small laptop screen; the
  narrowest tested width was 600 px. A second copy can be started next to the first.
- **Official packages:** the "Find official packages" card searches core and extra on this computer, by name,
  group and description, and forgives a typo or two (`fierfox` finds `firefox`); exact names come first, then
  names that start with the text, names that contain it, near misses, groups and descriptions. The databases are
  downloaded from the first mirror once (the same hourly cache as the resolve preview) when you first type; until
  then, or when the download fails, the card says so and offers Load now or Retry. Each row shows the version,
  repository, description, download and installed size; Add appends the name to the package list ("already
  added" otherwise). Under the list, a live total shows how many packages the installer will resolve and how much
  it will download, by the installer's own resolver, and the resolver's error when the list does not resolve.
- **Import from this system:** reads the names of the packages this computer installed explicitly (`pacman -Qqen`)
  and adds the ones core and extra have; the rest (other repositories such as multilib) are listed as skipped.
  Packages from outside the repositories (`pacman -Qqem`, mostly AUR) are only listed: pinning one needs its
  recipe reviewed in the AUR card. Nothing but package names is read.
- **AUR packages:** one compact card: search, a result row with Review, then the recipe with its automatic
  checks folded behind a count, risky lines highlighted, and one acknowledgement before "Pin and add". NetworkManager is added automatically when no network
  service is enabled, because the build needs one.
- `ARCHSTALLER_PAGE=<id>` (`system disk packages users services build scripts lua iso`) opens the window on a page.

## Manual smoke test

Manual check (no display server in the tests): start the app; open a preset from the menu and see the warning;
change the hostname and save, reopen the file; open Lua source, choose "Edit source", break the text and see
the error and its line, fix it and return to the forms; resolve dependencies; build an ISO from a preset and
see the stages finish; press `Ctrl+K` and run a command. The text widgets, the launcher and the builds were
checked this way on GTK 4.22 with GtkSourceView 5.20 in a dark theme (a light theme was looked at on one
page); keyboard-only use with a screen reader and other desktops was not.

See also [`gtk-gui.md`](../plans-implement/gtk-gui.md) (implementation spec) and [`raw-usb-flash.md`](../plans-implement/raw-usb-flash.md) (raw USB flashing).

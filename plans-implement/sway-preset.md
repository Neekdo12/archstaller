# Sway desktop preset implementation

**Status: written, not tested.** `configs/sway.lua` exists (paths below say `presets/`; the presets now live
in `configs/`, marked `-- archstaller: kind=preset`). Its packages were checked by name against the `core`/`extra`
databases of 2026-10-07; `waybar` needs `jack`, so it pins `jack = pipewire-jack`. The user config is a local
first-boot script, `configs/sway-config.sh` (embedded at build time, no download; `common.build` gained a `scripts`
field for it): it writes `~/.config/sway/config` for every regular user that has none and puts the same file in
`/etc/skel`. That config starts waybar as sway's `swaybar_command` (no second bar on reload), mako, wofi and foot,
binds brightness, volume (`wpctl`), media and screenshot keys, and includes `/etc/sway/config.d/*` so portals get
the session environment. Lock-on-idle is present but commented out until it is tested. Not done yet: steps 4-6
below (`check-presets`, QEMU install with ly starting sway, real hardware); the download size in the preset
tables is an estimate.

This document specifies a future Sway desktop preset for Archstaller.

Sway is an i3-compatible Wayland compositor. The intended result is a conventional Arch desktop that
uses Sway for the session, NetworkManager for networking, PipeWire for audio, and a small set of
Wayland utilities. Reuse the Archstaller installer and common preset defaults; do not add installer
kernel code or a new distribution backend.

## Existing patterns

Use `presets/i3.lua` as the closest starting point for a lightweight tiling desktop's package selection,
and `presets/hyprland.lua` for Wayland packages and the current optional user-configuration archive
pattern. Both call `presets/common.lua`, which supplies the Arch base packages, firmware, user, disk
selection, `ly`, and shared defaults.

Follow the existing preset conventions:

- Keep the config as an ordinary Arch package preset and resolve packages using the existing Arch
  `core`/`extra` databases.
- Use `common.build{ ... }` for shared defaults; do not duplicate the shared setup.
- Keep Sway configuration per-user under `~/.config/sway/`. Do not replace the system-wide Archstaller
  first-boot flow or add desktop-specific behavior to the installer kernel.
- Use only official Arch repository packages for the initial preset. Do not silently add AUR helpers or
  run an upstream installer script as root.

## Suggested package groups

Confirm exact package names and availability in current Arch `core`/`extra` databases before committing
an implementation; this list is a starting point, not a guarantee of repository state.

- Session and display: `sway`, `xorg-xwayland`, `mesa`, `polkit`.
- Login and session defaults: shared `ly` from `common.lua`; verify that the installed Sway session is
  discoverable and selectable through ly on the supported release. Do not assume that a Wayland session
  entry is installed without checking the package contents.
- Bar, launcher, terminal: `waybar`, `wofi`, `foot` (or another terminal available in the official
  repositories).
- Locking, idle, wallpaper: `swaylock`, `swayidle`, `swaybg`.
- Portals and notifications: `xdg-desktop-portal`, `xdg-desktop-portal-wlr`, `xdg-desktop-portal-gtk`,
  `mako`.
- Network and audio: `networkmanager`, `pipewire`, `pipewire-pulse`, `wireplumber`; add the same
  provider selection used by other desktop presets if package resolution requires `pipewire-jack`.
- Common Wayland actions: `wl-clipboard`, `grim`, `slurp`, `brightnessctl`, `playerctl`.
- Fonts and applications: `ttf-dejavu`, `noto-fonts`, and `firefox` as the default browser unless a
  different official-repository browser is selected.

Avoid pulling every utility by default. Each package should be used by the shipped config or represent a
clearly documented desktop baseline. Check size against the existing desktop presets and state a realistic
minimum disk size after package resolution.

## Sway configuration

Provide a useful, minimal config at `~/.config/sway/config` for each installed user. It should:

- Start a terminal, launcher, and bar with keybindings that reference packages actually included in the
  preset.
- Set a sensible modifier, close/focus/move/resize behavior, workspace switching, and output defaults
  without assuming a particular monitor name or resolution.
- Start `waybar`, `mako`, and any selected idle/portal helpers in a session-safe way. Avoid starting
  duplicate instances when Sway reloads its config.
- Include lock-on-idle only if the chosen `swayidle`/`swaylock` command is tested and resumes reliably.
- Include a clear, commented example for laptop brightness and media keys, but do not make hardware-
  specific commands fatal when those devices are absent.
- Preserve a normal path for users to edit the config after first boot.

Prefer a small project-owned config embedded with the preset over a mutable third-party URL. The
existing `user_archives` mechanism is not content-hash verified beyond HTTPS; if a remote bundle is
used, require an explicit trusted URL, disclose that it contains executable session configuration, and
allow the user to disable it. Do not point the preset at a personal or unversioned archive by default.

Create the destination with correct ownership. Verify the exact path semantics of the existing
`user_archives` extraction, which extracts each archive into every configured user's home. If that
mechanism cannot provide a project-owned static config safely, add the smallest hostcfg/firstboot
capability needed for a built-in config rather than improvising shell interpolation.

## Services and first boot

Enable `NetworkManager.service` using the existing `services` setting. Keep the common time-sync and ly
services. PipeWire is a per-user systemd session rather than a system service; do not add made-up global
service names. Verify the selected PAM/logind/session setup works with ly and Sway on a clean install.

Do not enable `seatd` or add the user to a privileged group speculatively. First verify Sway's supported
seat backend and the installed systemd-logind permissions for the target Arch package version; include
extra seat setup only if a real test demonstrates it is necessary.

## Implementation steps

1. Add `presets/sway.lua` using `common.build`, with a short first-line description and a package list
   resolved only from official Arch repositories.
2. Add the Sway user config through a safe, deterministic mechanism. Keep optional user customization
   separate from the core preset so an unavailable remote source never prevents installation.
3. Verify ly discovers the Sway Wayland session after first boot; if it does not, determine whether the
   session entry is supplied by the package or needs a small documented system entry. Test session
   startup as the configured user, not as root.
4. Run `cargo xtask check-presets` and confirm every package and provider resolves. Inspect total
   download size and adjust the minimum disk recommendation.
5. Build the Sway ISO and install it in QEMU with 3D acceleration where available. Confirm the system
   boots, reaches ly, starts Sway, launches the configured terminal/bar/launcher, and keeps a usable
   text-console fallback when graphics initialization fails.
6. Test on real Wayland-capable hardware with both an integrated GPU and an external display where
   possible. Verify keyboard input, multiple outputs, suspend/resume if claimed, Wi-Fi or wired
   networking as available, audio playback, screen locking, and clean logout.
7. Update `README.md`, `OVERVIEW.md`, preset build/check output, and GUI preset discovery (which scans
   `presets/*.lua`) when the actual Lua preset is implemented. Do not list Sway as available before
   package resolution and the install/login smoke test pass.

## Acceptance criteria

- The preset resolves against current Arch sync databases and uses no AUR-only dependency.
- `cargo xtask presets` builds its ISO and `cargo xtask check-presets` includes it.
- The installed image offers a working Sway session through ly and the user's config references only
  installed commands.
- Missing or unsupported graphics features do not make the installed system unbootable; the existing
  non-`kms` initramfs policy remains intact.
- The default config source is trusted and predictable. Any optional remote config is clearly disclosed
  and is not required for a usable Sway desktop.
- Documentation distinguishes implemented support from this implementation spec until the acceptance
  checks pass.

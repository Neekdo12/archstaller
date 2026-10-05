# Archstaler GUI Implementation Specification

Status: phases 1-4 implemented (`crates/hostcfg`, `gui/`): shared config/profile model, config editor, ISO build with
progress, Ventoy/folder copy with hash verification. Not implemented: direct flash (phase 5) and AUR (phase 6).
Deviations from this text: `extra-large` is reserved (rejected until a second optional build feature exists);
`--small`/`--super-small` still work as deprecated overrides; the GUI is cross-platform, raw flash will be Linux-only.

## Goal

Build a Rust desktop application for composing an Archstaler Lua configuration, validating it, building an ISO, and putting that ISO on removable media. The application is a host-side tool. It does not add an interactive interface to the booted installer kernel, which remains unattended.

The GUI must produce a normal, readable Lua config that can also be edited and built without the GUI. The CLI and GUI must use the same validation and build logic; the GUI must not maintain a second interpretation of the config.

## Recommended Technology

- **Desktop UI:** `egui` with `eframe`. It is Rust-native, works well for a focused configuration and progress tool, and avoids introducing a web frontend or a large GUI runtime.
- **File and folder dialogs:** `rfd` for selecting config files, output paths, and custom scripts.
- **Linux removable-media integration:** `zbus` to talk to UDisks2 over D-Bus. Use UDisks2 and its polkit authorization flow for device discovery, unmounting, and access to block devices; do not ask users to run the whole GUI as root.
- **Build execution:** invoke the existing `cargo xtask` command initially, with structured progress events added to its host-side interface. If sharing it as a Rust library becomes practical, put config/build orchestration in a host-only library used by both the CLI and GUI.

Keep the media-writing backend behind an interface so other operating systems can be added later without changing the GUI or Lua model. The first implementation should target Linux, matching the current build prerequisites and UDisks2 availability.

## Main User Workflow

1. Start a new config from a built-in starter or open an existing `.lua` file.
2. Edit system identity, disk selection, mirrors, users, services, kernel parameters, build profile, installer drivers, scripts, and packages in focused views.
3. Validate the config and resolve package choices. Display errors next to their fields and show warnings for risky settings, unresolved dependencies, or unavailable package sources.
4. Save the config as Lua. Show a preview of the generated file and preserve unknown/user-authored Lua only if the config editor can round-trip it reliably; otherwise offer a clear structured editor mode and a separate raw-Lua mode.
5. Build the ISO from the saved config. Show the exact command/config path, live output, build status, and resulting ISO path and size. A failed or cancelled build must not be presented as a usable ISO.
6. Choose one media action:
   - **Copy to Ventoy:** copy the ISO as a file onto the mounted Ventoy data partition. Do not repartition or format the stick.
   - **Flash ISO:** write the ISO image directly to a selected removable block device. This erases the selected device and requires a separate confirmation naming the device and its capacity.
7. Verify the result. For direct flash, flush writes and read back the written image (at minimum verify a full-image hash); for Ventoy, confirm the copied file size and hash.

Building and writing are separate explicit actions. Building an ISO must never implicitly select or modify a disk.

## Configuration Model

The Lua file remains the source of truth. Add a host-side `build` table and explicit lists for the requested selections. The following is an illustrative target shape, not a claim that these fields exist today:

```lua
return {
  build = {
    profile = "super-small",
  },

  installer_drivers = { "virtio-blk", "nvme", "e1000" },
  packages = { "base", "linux", "grub" },
  scripts = {
    { id = "enable-sshd", enabled = true },
  },
  aur_packages = {},

  -- Existing fields such as disk, mirrors, users, and services remain here.
}
```

The schema and versioning rules must distinguish:

- **Build-host settings**, such as the compiler profile, enabled installer features, output path, and optional AUR build inputs. These control ISO production and are not silently treated as kernel install-time values.
- **Installer settings**, such as target disk selection, packages, users, services, scripts, and mirrors. These are validated and serialized into the ISO for the installer.

Validation should be shared with `xtask`, report actionable field paths, and reject unknown profile/driver/script identifiers. The saved Lua should be deterministic and readable: stable ordering for generated lists, quoted strings, and no plaintext passwords. Password entry should use a confirmation field and save only a supported password hash, never the plaintext value.

### Build Profiles

Use the profile names `super-small`, `large`, and `extra-large` in Lua, and make `super-small` the default for new configurations and for builds without an explicit profile.

- `super-small` is the size-optimized installer build. Initially map it to the existing `--small`/Cargo `small` build behavior.
- `large` is the regular release installer build, matching today's default build behavior.
- `extra-large` is an explicitly defined superset of `large`, enabling only catalogued optional build features. Define the feature set and its size impact before exposing this choice; do not imply that it installs a larger Arch system. Package selection remains controlled by `packages` and related package-source fields.

Move profile selection from `--small` into Lua. During migration, retain the old CLI switch as a deprecated override with a warning, then remove it only after documented compatibility period. Define precedence explicitly: a CLI override, when supplied, wins temporarily; otherwise the config value wins; if neither is supplied, use `super-small`. Remove the misleading `--super-small` alias once the Lua profile is supported.

Show the profile's build-time effects and an ISO size estimate where available. The GUI must not silently turn on destructive self-tests, debug behavior, or unrelated USB features as a side effect of choosing a size profile.

### Installer Drivers

Expose a maintained catalogue of supported installer drivers, grouped by device class and marked with availability, prerequisites, and test status. The GUI must distinguish installer-kernel drivers from packages installed into the resulting Arch system; selecting a userspace driver package does not enable an installer driver.

Initially, driver selection may map to compile-time features or other explicit build inputs. Preserve the minimum drivers required for boot and installation, and prevent configurations that remove every usable storage or network path unless the user explicitly selects a supported alternative. Do not offer unsupported or unimplemented drivers as if they worked. Reflect selected drivers in the generated Lua and the build summary.

### Scripts

Provide a catalogue of built-in first-boot actions and allow selecting exact script IDs with documented parameters. For custom scripts, support a local file embedded into the build input and optionally a remote HTTPS source only when pinned by a SHA-256 digest. Show the script source, execution phase, privileges, and digest before build.

Scripts are executable code, not harmless config values. Never execute a selected target-system script on the build host. Run selected scripts only at a documented target-system phase, with clear root/non-root semantics, deterministic ordering, bounded output, and failure policy. Require a prominent acknowledgement for custom scripts that run as root. Reject unsafe archive paths and avoid interpolating Lua values into shell source; pass data through validated files or argument arrays.

### Packages and AUR

Keep official repository packages in the existing `packages` list and make individual package/group selection visible, including the resolved dependency set and ambiguous provider choices. Validate against the same package metadata and dependency rules used by the build/install workflow.

AUR packages require a separate host-side build pipeline; the current installer resolver covers Arch `core` and `extra`, and the `no_std` installer cannot run `makepkg` or arbitrary PKGBUILDs. Do not fetch/build an AUR package on the installed target during the unattended kernel install.

For an initial AUR-capable design:

- Pin each AUR source to a reviewed commit and record package name, version, source revision, and artifact SHA-256 in a build manifest.
- Build in an isolated, unprivileged environment with a clean build root. PKGBUILDs and their dependencies can execute arbitrary code; warn that sandboxing reduces, but does not eliminate, this risk.
- Resolve official dependencies separately and build AUR runtime/build dependencies in a reproducible, ordered plan. Present the complete plan before starting.
- Add an explicit staging path for the resulting package artifacts: include them in a build artifact bundle and teach the installer to place them in `/var/cache/pacman/pkg` for the existing first-boot `pacman -U` phase, or provide a clearly versioned local repository. The current ISO has no such package cache. Do not claim Arch repository signatures cover locally built artifacts. Verify their recorded hashes and define whether a user-managed signing key is supported.
- Keep AUR disabled unless the user explicitly enables it. Show source, maintainer metadata when available, PKGBUILD revision, build logs, and the trust warning.

The Lua representation should identify AUR packages and pinned revisions rather than embedding credentials or accepting an unpinned moving branch as a reproducible input.

## ISO and Removable Media

### Build

The GUI should initially delegate to `cargo xtask build --config <saved.lua> --out <path>` and consume exit status plus structured progress output. Do not parse human-readable log wording as the long-term progress API. The build must use the same config validation, keyring generation, package resolution, and ISO assembly as a CLI build.

Keep temporary files and AUR artifacts in a per-build workspace. Cancellation must terminate the active child process, clean only that build's temporary files, and retain useful logs. Never delete or overwrite a user-selected output file without an explicit overwrite confirmation.

### Direct Flash

Direct flash overwrites the entire selected device. The GUI must:

- List device model, path, capacity, connection/removable status, and mounted partitions; refresh and revalidate identity immediately before writing.
- Exclude the system disk and mounted devices by default. Require explicit selection and a destructive confirmation that names the exact device and capacity.
- Unmount its partitions through UDisks2, acquire the required polkit authorization, write sequentially with progress, flush, and verify the result.
- Detect device disappearance, short writes, I/O errors, and cancellation. Report whether the medium may be incomplete; never claim success unless verification passes.
- Never run shell-formatted commands such as `dd ...` assembled from user/device strings, and never request blanket root privileges for the application.

The installer currently has no USB mass-storage driver; this workflow writes the ISO from the host and does not imply that the running installer can install onto a USB disk.

### Ventoy

When a mounted Ventoy data partition is detected, offer **Copy ISO to Ventoy** as the non-destructive path. Detection should use the mounted filesystem/partition context and Ventoy markers where available; a label alone is not sufficient proof. Let the user choose the destination directory and reject insufficient free space.

Copy the complete ISO as a regular file, preserving its filename, then flush and verify its hash. Do not format, repartition, install/update Ventoy, or write the ISO to the Ventoy device as a raw image. If Ventoy cannot be reliably identified, show a generic mounted-volume copy option only with an explicit destination choice; never silently fall back to direct flash.

## Error Handling and Safety

- Separate config validation, package resolution, build, Ventoy copy, and raw flash states in the UI. Preserve logs and identify the failed stage.
- Require confirmation before enabling `disk.auto_largest`, selecting custom root scripts, using AUR build inputs, overwriting an ISO file, or flashing a block device.
- Do not store plaintext passwords, polkit secrets, or signing-key material in the config or logs.
- Make the selected config path, effective build profile, package sources, output ISO, and destination device/volume visible before each irreversible operation.
- A crash or application exit must not leave the UI showing a successful flash when verification has not completed.

## Suggested Implementation Phases

1. **Shared config and profile:** define/validate Lua host build settings; add `super-small`, `large`, and a concretely scoped `extra-large`; migrate the CLI with a deprecation path; add focused config/build tests.
2. **Configuration GUI:** implement config loading/saving, structured editors, driver/script/package catalogues, Lua preview, validation, and package-resolution preview. Do not add media writing in this phase.
3. **ISO build:** invoke the same `xtask` build path, stream progress, handle failure/cancellation, and verify the output exists and is readable.
4. **Ventoy copy:** add mounted-volume selection, free-space checks, copy, and hash verification. This is the first media-writing feature because it does not erase the Ventoy stick.
5. **Direct flash:** add UDisks2 discovery, polkit authorization, guarded raw writes, read-back verification, and hardware tests on disposable media.
6. **AUR pipeline:** implement only after artifact provenance, isolation, dependency ordering, first-boot installation, and local artifact verification are designed and tested.

Do not combine the first GUI release with AUR execution or automatic raw flashing. Both expand the trust and data-loss surface substantially.

## Acceptance Criteria

- A new project defaults to `super-small`; saved Lua can reproduce a GUI build from the CLI without `--small`.
- The GUI and CLI reject the same invalid config and package selections.
- Saving and reopening a generated config preserves all supported settings and exact driver, script, and package selections.
- Build output names the config and profile used and reports actionable errors without hiding the underlying log.
- Ventoy copy leaves the partition layout unchanged and verifies the copied ISO.
- Direct flash requires explicit device-specific confirmation, does not target the system disk by default, and verifies written bytes before reporting success.
- AUR artifacts are pinned, isolated-build outputs with recorded hashes and are never represented as official Arch-signed packages.
- The installer kernel remains unattended; no GUI dependency is added to `kernel/`, `no_std` crates, or the boot path.
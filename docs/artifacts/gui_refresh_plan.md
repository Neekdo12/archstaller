# Implementation Plan: Archstaller GUI Refresh & Feature Documentation

## Goal Description
Refactor the Archstaller GTK4 GUI into 5 cohesive configuration tabs and move document actions (TOML editing, ISO creation/flashing, autoloading) into the top header bar, as outlined in [docs/gui-refresh.md](file:///home/kubik/scr/rust/archstaler/docs/gui-refresh.md). 

Simultaneously, establish the `docs/planned-gui-features/` directory containing dedicated specifications for future roadmap features that are **not currently implemented in the backend/installer engine** (such as package download testing, advanced multi-distro partitioning, curl application installers, user-specific services, and Wi-Fi configuration).

All implementation in this task is **GUI-only** (`gui/` crate and associated docs).

---

## User Review Required

> [!IMPORTANT]
> **Scope Confirmation**:
> - **In-Scope (GUI Implementation)**:
>   1. Reorganize navigation into 5 core tabs: **System**, **Disk**, **Packages**, **Users**, and **Build and drivers**.
>   2. Move **Edit TOML** and **Build & Flash ISO** out of the sidebar and into distinct action buttons on the `gtk::HeaderBar`.
>   3. Add the **Autoload** pop-up dialog button on the header bar (replacing the reinstall preset with a selective hardware/config import).
>   4. Move **Bootloader** (GRUB, systemd-boot, Limine) to the **Disk** tab.
>   5. Move **System-wide Services**, **Display Manager**, **Post-Install Scripts**, and **Root Account / Password** to the **System** tab.
>   6. Unify regular user configuration in **Users** (seamless home subvolume and dotfiles).
>   7. Refactor package and overlay listings into compact, searchable lists with `gtk::SearchEntry`.
>   8. Reposition **Help** as a header bar action (`help-browser-symbolic`) so documentation remains instantly accessible without occupying a configuration tab.
>   9. Update `gui/src/model.rs` (`Area` and `page_of`) and test coverage to match the 5-tab structure.
> - **Documented Only (`docs/planned-gui-features/`)**:
>   Features requiring future backend/kernel/installer engine work (package download tester, multi-distro partitioning, curl script installers, user-level systemd services, Wi-Fi configuration, Ventoy flashing, graphical QEMU E2E launcher) will be documented as modular `.md` files without altering backend crates.

---

## Open Questions

> [!NOTE]
> None currently. The separation between GUI-only refactoring and future backend feature documentation is clear.

---

## Proposed Changes

```
┌────────────────────────────────────────────────────────────────────────┐
│ Archstaller HeaderBar                                                 │
│ [New] [Open] [Save] [Presets ▾] [Autoload]   [Edit TOML] [Build ISO] (?)│
├───────────────┬────────────────────────────────────────────────────────┤
│ CONFIGURATION │                                                        │
│ 🖥️  System    │ Hostname, Timezone, Locale, Keymap                    │
│ 💾 Disk       │ Bootloader (GRUB/systemd-boot/Limine), Disk, Fs, ESP   │
│ 📦 Packages   │ Searchable Mirrors, Pacman Packages, AUR Packages      │
│ 👤 Users      │ User Accounts, Home Subvolume, Dotfiles (Seamless)     │
│ ⚙️  Build & Drv│ Drivers Catalog, Build Profile (Release/Debug), Flags  │
└───────────────┴────────────────────────────────────────────────────────┘
```

---

### Component 1: Documentation Layer (`docs/planned-gui-features/`)

Create modular specification files in `./docs/planned-gui-features/` cataloging future features that require backend engine support:

#### [NEW] `docs/planned-gui-features/01-package-download-tester.md`
- Specification for an in-RAM package download tester: downloading sample packages to verify network throughput, mirror responsiveness, and integrity before running a full installation.

#### [NEW] `docs/planned-gui-features/02-advanced-disk-partitioning.md`
- Specification for future full partitioning capabilities: dual-boot support with existing Linux/Windows installations, custom partition sizing, and dedicated btrfs subvolumes/partitions for `/home`, `/var/log`, etc.

#### [NEW] `docs/planned-gui-features/03-curl-app-installers.md`
- Specification for curl-based application installers (e.g. oh-my-zsh, rustup, custom git setups) executed during post-install firstboot.

#### [NEW] `docs/planned-gui-features/04-user-specific-services-and-scripts.md`
- Specification for user-level systemd units (`systemctl --user`), per-user login scripts, and user-isolated package environments.

#### [NEW] `docs/planned-gui-features/05-wifi-configuration.md`
- Specification for GUI-based Wi-Fi network scanning, SSID selection, and credential storage (to align with `plans-implement/wifi.md` once kernel Wi-Fi driver support arrives).

#### [NEW] `docs/planned-gui-features/06-ventoy-flashing.md`
- Specification for direct Ventoy USB partition detection and ISO file placement alongside raw disk block writing.

#### [NEW] `docs/planned-gui-features/07-graphical-qemu-e2e.md`
- Specification for launching QEMU with a graphical display window (`-display gtk` or SDL) directly from the GUI to visually watch the installer run.

#### [NEW] `docs/planned-gui-features/08-enhanced-diagnostics-and-logging.md`
- Specification for runtime log verbosity adjustment, kernel ring buffer export, and interactive installer error diagnostics.

---

### Component 2: GUI Navigation & App Shell (`gui/`)

#### [MODIFY] [gui/src/app.rs](file:///home/kubik/scr/rust/archstaler/gui/src/app.rs)
- Update `Page` enum to the 5 configuration pages + 2 action views:
  ```rust
  #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
  pub enum Page {
      System,
      Disk,
      Packages,
      Users,
      Build,
      // Action views (opened from header bar)
      Toml,
      Iso,
      Help,
  }
  ```
- Update `PAGES` constant: only the 5 configuration pages (`System`, `Disk`, `Packages`, `Users`, `Build`) appear in the sidebar navigation.
- In `App::new()`:
  - Add HeaderBar action buttons:
    - **Edit TOML** toggle button (`document-edit-symbolic`).
    - **Build ISO / Flash** action button (`media-optical-symbolic` with highlighted styling).
    - **Autoload** button (`system-search-symbolic` or `view-refresh-symbolic`).
    - **Help** button (`help-browser-symbolic`).
  - Configure the Autoload button to open a modal dialog with granular checkboxes (Detect NIC, Detect CPU Microcode, Detect Storage, Apply Minimal Base).
  - Configure Edit TOML to toggle between form view and TOML view, validating on exit.

#### [MODIFY] [gui/src/model.rs](file:///home/kubik/scr/rust/archstaler/gui/src/model.rs)
- Update `Area` enum or `page_of` function:
  ```rust
  pub fn page_of(area: Area) -> Page {
      match area {
          Area::System | Area::Scripts | Area::Services => Page::System,
          Area::Disk => Page::Disk,
          Area::Packages => Page::Packages,
          Area::Users => Page::Users,
          Area::Build => Page::Build,
          Area::Other => Page::System,
      }
  }
  ```
- If a validation message pertains to `bootloader`, route it to `Page::Disk`.
- Add unit tests for area and page mapping.

---

### Component 3: Consolidated Pages (`gui/src/pages/`)

#### [MODIFY] [gui/src/pages/system.rs](file:///home/kubik/scr/rust/archstaler/gui/src/pages/system.rs)
- Expand the System tab into structured sections:
  1. **Identity & Localization**: Hostname, Timezone, Locale, Keymap.
  2. **Root Account**: Root password hash entry, root account enablement switch.
  3. **Display Manager**: Display manager selector (None, Ly, SDDM, GDM, LightDM).
  4. **System Services & Kernel Parameters**: Systemd services list (chrony, iptables, sshd, etc.) and kernel command-line parameters.
  5. **Post-Install Scripts**: Shell scripts table/list with chroot toggle and source editor (consolidated from `scripts.rs`).
  6. **Filesystem Overlays**: File overlays and archives list.

#### [MODIFY] [gui/src/pages/disk.rs](file:///home/kubik/scr/rust/archstaler/gui/src/pages/disk.rs)
- Add the **Bootloader** selector (GRUB, systemd-boot, Limine) to the top of the Disk page.
- Group with: Target disk selection (auto-largest or disk model), confirm serial, ESP size (MiB), filesystem type (ext4, btrfs, xfs, f2fs), and btrfs subvolume editor.

#### [MODIFY] [gui/src/pages/packages.rs](file:///home/kubik/scr/rust/archstaler/gui/src/pages/packages.rs)
- Replace giant scrollable textviews with compact, searchable list widgets:
  - `gtk::SearchEntry` for live filtering of packages.
  - A scrollable `gtk::ListBox` with removable package rows.
  - An "Add Package" entry with completion or enter-key addition.
  - A dedicated section for AUR packages & Git repositories with the same searchable pattern.

#### [MODIFY] [gui/src/pages/users.rs](file:///home/kubik/scr/rust/archstaler/gui/src/pages/users.rs)
- Remove Root Password (moved to System).
- Streamline the user card: Username, UID, Shell, Sudo toggle, Additional Groups, Password, and seamless Home Subvolume & Dotfiles URL/branch/commit in a single integrated section.

#### [DELETE] [gui/src/pages/scripts.rs](file:///home/kubik/scr/rust/archstaler/gui/src/pages/scripts.rs)
- Remove standalone `scripts.rs` as its logic is fully absorbed into `system.rs`.

#### [DELETE] [gui/src/pages/services.rs](file:///home/kubik/scr/rust/archstaler/gui/src/pages/services.rs)
- Remove standalone `services.rs` as bootloader moved to `disk.rs`, and display manager/services/kernel params moved to `system.rs`.

---

### Component 4: Documentation Synchronization

#### [MODIFY] [OVERVIEW.md](file:///home/kubik/scr/rust/archstaler/OVERVIEW.md)
- Update the GUI section to describe the 5-tab configuration model, top bar action buttons, and reference the new `docs/planned-gui-features/` directory.

---

## Verification Plan

### Automated Tests
1. Unit tests for `gui::model` area and page routing:
   ```bash
   cargo test -p archstaller-gui
   ```
2. Full workspace unit test suite:
   ```bash
   cargo test --workspace
   ```
3. Clippy compliance across all targets:
   ```bash
   cargo clippy --workspace --all-targets
   ```

### Manual Verification
1. Launch the GUI:
   ```bash
   cargo xtask gui
   ```
2. Verify visual layout:
   - Sidebar contains exactly 5 tabs under "CONFIGURATION": System, Disk, Packages, Users, Build & drivers.
   - HeaderBar displays "Edit TOML", "Build & Flash", "Autoload", and Help buttons.
   - Navigating to **System** displays Identity, Root Account, Display Manager, Services, Scripts, and Overlays.
   - Navigating to **Disk** displays Bootloader selection alongside Disk, ESP size, and Filesystem.
   - Navigating to **Packages** allows searching/filtering packages with `gtk::SearchEntry`.
   - Navigating to **Users** shows unified user accounts with integrated dotfiles/home settings without the root password field.
   - Clicking **Edit TOML** switches smoothly into the TOML editor, and leaving returns to the previous configuration tab.
   - Clicking **Autoload** opens the selection popup.

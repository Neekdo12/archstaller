# GTK desktop GUI implementation

**Status: implemented.** `gui/` is a GTK 4 application with a GtkSourceView 5 Lua editor; the egui front end
is gone. Decisions and differences from the text below:

- **Platform:** Linux only. Windows and macOS support of the old GUI was dropped on purpose (the Lua editor and
  the volume handling use `lsblk`, `udisksctl` and GtkSourceView).
- **No libadwaita:** plain GTK 4 only (`HeaderBar`, `ListBox` sidebar, `Stack`); the theme is the system's and the
  status colours follow its lightness. Narrow windows hide the sidebar (a header toggle brings it back).
- **Phases 1-5 are done**, including the Ctrl+K / `/` launcher, the source and form modes, completion, diagnostics
  and the verified Ventoy copy. The AUR group, the home-directory zip and the first-boot script pages were ported
  as they were.
- **Verification:** `cargo test -p archstaler-gui` covers the model (including source mode and line lookup) and the
  completion logic without a display. The windows, the launcher, the editor and a real ISO build from a preset were
  exercised by hand under X11 with a dark theme; see "Manual check" in `README.md`. Not exercised: a light theme,
  Wayland, a screen reader, high-contrast mode, and a build onto a real Ventoy stick.
- **Not done:** a separate controller layer (the pages write through `App::edit`), completion while typing (only on
  Ctrl+Space), package-name suggestions beyond the last resolution (no sync database is kept loaded), raw flashing.

This is the implementation spec for replacing the current egui desktop frontend with a GTK application.
The GUI remains a host-side tool for editing Archstaler configs, validating them, building installer
ISOs, and copying an ISO file to a Ventoy data volume. It is not an installer UI and does not flash raw
devices.

The visual direction should feel like a focused Linux desktop utility: use Thunar as inspiration for
clear location/navigation structure and file-oriented workflows, and Wofi as inspiration for a fast,
keyboard-first command launcher. Borrow interaction patterns, not branding, assets, code, or exact
layouts.

## Existing behavior to preserve

The current application lives in `gui/` and uses eframe/egui. Keep its UI-independent modules and
contracts where possible:

- `gui/src/model.rs` owns the editable document: shared `config::Config`, host-only `HostConfig`, Lua
  open/save, preset loading, and validation through the same `hostcfg` code as the CLI. It must remain
  independent of GTK so its tests and config rules stay reusable.
- `gui/src/build.rs` starts `cargo xtask build`, receives JSON progress events and log output, supports
  cancellation, and uses a per-build workspace. Preserve those semantics and ensure cancellation only
  removes the active build's workspace.
- `gui/src/media.rs` discovers mounted volumes, identifies Ventoy by its marker files, and copies an ISO
  as a regular file with free-space checks, explicit overwrite, cancellation, and read-back SHA-256
  verification. Keep `MediaTarget` as the boundary for future media operations; do not add raw flashing
  in this migration.
- `hostcfg` remains the owner of validation, preset parsing, package resolution, progress events, and
  config generation. Do not duplicate its rules in widget callbacks.

Preserve the current workflows: new config; open/save Lua; create a document from a repository preset;
edit system, disk, mirrors/packages, users, services/kernel options, installer build/drivers, and scripts;
view/edit Lua; resolve packages; build an ISO; inspect build progress/logs; and copy a completed ISO to a
Ventoy volume.

## GTK stack and architecture

Use Rust GTK bindings (`gtk4` crate) with GTK 4, and GtkSourceView 5 (`sourceview5` crate) for the Lua
source editor. Use `gio`/`glib` actions and application lifecycle conventions instead of maintaining a
parallel custom event framework. Evaluate `libadwaita` for adaptive navigation and standard dialogs, but
keep the app's content styling compatible with ordinary GTK themes and do not require GNOME Shell.

Keep the UI and domain logic separated:

1. `model`, `build`, and `media` stay free of GTK types.
2. GTK widgets render model state and dispatch user intents. A controller/coordinator layer may be added
   for document transitions and asynchronous operations, but it must not become a second config model.
3. All widget access stays on the GLib main context. Package resolution, ISO builds, volume scanning,
   and media copying run asynchronously; workers send typed messages back to the main context, and the
   UI updates from those messages.
4. Use GTK/GIO file choosers, actions, shortcuts, and dialogs where they fit. Avoid blocking the main
   thread for filesystem or process work.
5. Do not add GTK dependencies to `kernel/`, `config/`, or `crates/*`; GUI dependencies remain under
   `gui/`.

Keep GUI actions explicit and testable. Important actions should be GActions (or an equivalent GTK action
with a stable name) so menus, buttons, and the command launcher invoke the same implementation.

## Window and interaction design

Use a resizable application window with a GTK header bar and a two-part workspace:

- A compact, persistent navigation sidebar groups configuration areas and workflow destinations. It
  should behave like a file manager's location pane: clear current selection, predictable ordering, and
  enough width for labels without wasting the editing area.
- The main pane shows one focused page at a time. Keep page titles and primary actions visible while
  scrolling long forms. Use a bottom status area or transient notifications for save/build/copy results,
  not a stack of decorative cards.
- Put document actions (new, open, save, save as, presets) in the header/menu. Keep build and media
  actions close to the ISO workflow, with enabled/disabled states derived from validation and operation
  state.
- Provide a Wofi-inspired command launcher opened by `Ctrl+K` or `/`. It searches available actions and
  destinations, displays keyboard hints, supports arrow-key navigation and Enter/Escape, and never
  executes a destructive action without the same explicit confirmation as its normal UI path.
- Use familiar GTK controls: switches/check buttons for booleans, dropdowns for bounded choices, numeric
  spin buttons for numeric values, list rows for users/services/packages, and dialogs for file selection
  and confirmation. Use text editors only for genuinely free-form Lua, package lists, scripts, or
  multi-line fields.
- Use responsive/adaptive layout so the sidebar can collapse on narrow windows and forms remain usable
  without horizontal scrolling. Keep keyboard focus visible and labels associated with their controls.

Do not make the whole app look like a file manager or a launcher. These references guide navigation and
command discovery; the product remains a configuration/build utility.

## Styling and theming

Use GTK's normal theme and widget rendering as the foundation. Add a small application stylesheet only
for Archstaler-specific hierarchy, spacing, selected navigation, status severity, and progress emphasis.
Use named CSS colors/design tokens consistently; avoid a large global stylesheet that fights the active
GTK theme or hard-codes every widget's colors.

Support the system light/dark preference and GTK theme. If an in-app accent/theme selector is added,
limit it to a small supported set and apply it through the app stylesheet; do not replace native control
behavior. Keep contrast, focus indicators, high-contrast preferences, and reduced-motion settings
usable. Icons should come from the system icon theme or bundled project-owned assets, with text labels
or tooltips where an icon's meaning is not obvious.

## Page and workflow requirements

### Configuration

- Show a clear dirty indicator and ask before discarding unsaved changes.
- Presets are templates: selecting one creates an unsaved document and must make the largest-disk
  erase default conspicuous before the user builds an ISO.
- Show validation errors next to the relevant page/control and retain a summary that navigates to the
  first problem. Use `Model::problems()` as the source of truth.
- Keep foreign/non-generated Lua editable as raw text without silently overwriting it with form output.
- Provide an explicit Lua source-edit mode for generated configs too; see the Lua editor requirements
  below. Never silently discard source edits when switching between source and form modes.
- Password entry and confirmation stay masked. Only the generated password hash is stored in config.
- For scripts that run as root, retain an explicit acknowledgement that shows the script identity/source
  and its privilege implications.

### Packages

- Keep mirrors and explicitly requested packages editable, and show package resolution as asynchronous
  work with progress/busy state, errors, ambiguity choices, and the resolved package list/size.
- Package resolution and validation continue through `hostcfg`; the GTK frontend must not invent
  package metadata or silently change provider selections.

### Lua source editor

- Replace the read-only generated-Lua preview with an embedded GtkSourceView 5 editor. Generated
  configs and foreign/raw Lua files can both be edited directly in the application.
- Enable the Lua language definition for syntax highlighting. Show line numbers and current-line
  highlighting, preserve indentation, support undo/redo and editor search, and use a style scheme that
  follows the selected GTK light/dark preference. If the Lua language definition is unavailable, keep
  editing functional in plain-text mode and show a non-blocking notice.
- Keep source and form editing as explicit modes. Entering source mode serializes the current form once
  and edits that text buffer. While source mode is active, form controls are unavailable so there is
  only one editable representation. Re-evaluate edited text with the same `hostcfg` Lua loading and
  validation path used by the CLI. Return to form mode only after successful evaluation; on failure,
  stay in the editor, preserve all text, and show the parser/validation message. Switching modes with
  unsaved edits requires an explicit save/discard decision.
- Show parser and config-validation diagnostics beside or below the editor. Include a source location
  when the parser provides one; do not invent line/column locations for errors that lack them. Clicking
  a located diagnostic moves the editor cursor to that line.
- Add lightweight, non-modal completions: Lua keywords and standard syntax where supported; known
  config field names and documented values; build-profile names, installer-driver IDs, built-in script
  IDs, and package/group names from an already loaded sync database. Reuse `hostcfg` catalogues and the
  active package resolution data as sources of truth rather than maintaining duplicate lists in the
  GTK layer.
- Offer completion on `Ctrl+Space` and optionally while typing, with a small unobtrusive popup and
  short descriptions. Accept only on explicit selection/Tab/Enter; Escape dismisses it. Do not insert
  text automatically, generate whole config blocks without consent, or add a full LSP process for this
  feature.
- Completion and highlighting are editor assistance, not validation. The CLI/hostcfg validation result
  remains authoritative, and completion must never make an invalid config appear accepted.

### Build ISO

- Disable build when validation fails or another build is active. Explain the blocking validation issue
  rather than leaving a dead button.
- Show named build stages, package/byte progress when provided, elapsed/current status, and a scrollable
  log. Keep the log selectable and allow opening its location.
- Provide cancellation with an explicit in-progress state. A completed ISO is offered only after the
  build process succeeds and `build::usable` confirms its recorded size and readability.

### Copy ISO

- Refresh mounted-volume data on demand and before a copy. Show mount path, filesystem, available space,
  and Ventoy detection evidence; do not treat a volume label alone as proof of Ventoy.
- Explain that the operation copies an ISO file to a mounted Ventoy data volume, not a raw flash. Show
  overwrite confirmation, copy and verification progress separately, and the final destination/hash.
- Preserve cancellation and cleanup behavior from `FolderCopy`.

## Implementation phases

1. **GTK shell:** replace eframe startup with `GtkApplication`, GTK window/headerbar, navigation, theme
   integration, and a minimal page host. Keep the existing eframe app buildable only if doing so is
   useful for a short transition; do not keep two supported frontends long term.
2. **Config editing:** port document actions and the System, Disk, Users, Services, Build, and Scripts
   pages. Reuse `Model`, validation, preset discovery, and Lua behavior unchanged.
3. **Packages and Lua editor:** port package/mirror editing, asynchronous resolution, ambiguity display,
   generated and foreign Lua source editing, highlighting, completion, and diagnostics.
4. **Build and media:** integrate build event/log handling, cancellation, completed ISO state, volume
   selection, overwrite confirmation, and verified Ventoy copy.
5. **Polish and packaging:** keyboard launcher/actions, responsive layout, accessibility, theme checks,
   startup/error handling, and platform packaging/documentation.

Before phase 1, verify GTK4 development/runtime availability and decide which host platforms remain
supported. GTK is the intended native Linux experience, but do not silently drop Windows/macOS support
currently provided by the Rust GUI dependencies; record a deliberate platform decision and test the
chosen targets.

## Validation and acceptance

- Keep `cargo test -p archstaler-gui` passing, including existing model, build, and media tests. Add
  focused tests for any new controller/state transitions without requiring a display server.
- Add a smoke test or documented manual check that launches the app, opens a config, creates a preset
  document, saves/reopens Lua, and shows validation errors at the correct page.
- Exercise package resolution and build progress while interacting with the window; verify the UI stays
  responsive and no GTK widget is accessed from a worker thread.
- Exercise successful, failed, and canceled builds; ensure only the active build workdir is removed on
  cancellation and failed builds do not expose a usable ISO.
- Exercise Ventoy detection, insufficient free space, overwrite refusal/approval, cancellation, and
  read-back verification for media copy.
- Check keyboard-only operation, focus order, screen-reader labels, narrow-window layout, system
  light/dark themes, and the supported Linux desktop environments.
- Update `README.md` with GTK build/runtime requirements and launch instructions, and update
  `OVERVIEW.md` to distinguish the implemented frontend from this migration spec.
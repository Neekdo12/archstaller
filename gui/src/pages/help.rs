//! Help and reference page: guide to presets, bootloaders, display managers, filesystems,
//! hardware support, and keyboard shortcuts.
use crate::app::App;
use crate::ui;
use gtk::prelude::*;
use std::rc::Rc;

pub fn build(_app: &Rc<App>, content: &gtk::Box) {
    ui::heading(
        content,
        "Help & Reference",
        "Documentation, configuration guide, and system reference.",
    );

    // 1. Quick Start Guide
    let c = ui::card(content, Some("Quick Start Guide"));
    ui::hint(
        &c,
        "Archstaller automates creating customized, reproducible Arch Linux installations without manual chrooting.",
    );
    let steps = [
        ("1. Presets & Autoload", "Click 'From preset' or 'Reinstall This Computer' to start from a tested baseline. You can also autoload hardware, users, and Wi-Fi configurations from your running system."),
        ("2. Disk & Partitions", "Select your target disk (by serial or largest disk), configure the ESP partition size, and choose your root filesystem."),
        ("3. Packages & Desktop", "Select official packages and pinned AUR packages. Desktop presets include environments like Hyprland, Sway, KDE Plasma, or i3."),
        ("4. Bootloader & Services", "Choose between GRUB, systemd-boot, or Limine. Enable network services and display managers (Ly, greetd, SDDM, LightDM)."),
        ("5. Build & Flash", "Go to 'Build ISO' to generate a self-contained installation medium. Flash it to a USB drive or launch directly in QEMU to test."),
    ];
    for (title, desc) in steps {
        let b = ui::vbox(2);
        b.append(&ui::strong(title));
        b.append(&ui::dim(desc));
        c.append(&b);
    }

    // 2. Bootloaders
    let c = ui::card(content, Some("Bootloaders"));
    ui::hint(
        &c,
        "Archstaller supports three distinct bootloader engines configured during the first boot.",
    );
    let loaders = [
        ("GRUB (GNU GRand Unified Bootloader)", "Default bootloader supporting both BIOS (MBR/GPT) and UEFI. Detects other OS installs and supports complex partition schemes."),
        ("systemd-boot (UEFI Only)", "Minimalist, high-performance UEFI boot manager built into systemd. Simple Type #1 Boot Loader Specification entries, zero background daemons."),
        ("Limine (BIOS + UEFI)", "Modern, lightweight, and ultra-fast bootloader supporting both BIOS and UEFI. Clean declarative limine.conf syntax with direct Linux protocol booting."),
    ];
    for (title, desc) in loaders {
        let b = ui::vbox(2);
        b.append(&ui::strong(title));
        b.append(&ui::dim(desc));
        c.append(&b);
    }

    // 3. Display Managers & Archstaller Branding
    let c = ui::card(content, Some("Display Managers & Branding"));
    ui::hint(
        &c,
        "Supported display managers feature permanent ARCHSTALLER branding across reboots and package upgrades.",
    );
    let dms = [
        ("Ly (TUI Display Manager)", "Runs on TTY2 (leaving TTY1 for standard login). Features a permanent ANSI/DurDraw movie background of the ARCHSTALLER ASCII art (/etc/ly/archstaller.dur)."),
        ("greetd & tuigreet", "Modern, lightweight console greeter. Automatically displays the ARCHSTALLER ASCII art banner from /etc/issue via the '--issue' flag."),
        ("SDDM (Simple Desktop Display Manager)", "Standard Qt/KDE graphical login manager. Configured with the dark Archstaller branding theme and user avatar support in /etc/sddm.conf.d/."),
        ("LightDM & GTK Greeter", "Lightweight GTK graphical display manager. Configured in /etc/lightdm/lightdm-gtk-greeter.conf with the Archstaller vector SVG logo (/usr/share/pixmaps/archstaller.svg)."),
    ];
    for (title, desc) in dms {
        let b = ui::vbox(2);
        b.append(&ui::strong(title));
        b.append(&ui::dim(desc));
        c.append(&b);
    }

    // 4. Filesystems
    let c = ui::card(content, Some("Filesystems"));
    let fss = [
        ("Btrfs", "Copy-on-write filesystem with subvolumes (@ for root, @home for user data) and transparent zstd compression. Ideal for snapshotting and rollbacks."),
        ("Ext4", "The tried-and-true default Linux filesystem. Exceptional reliability, fast journaling, and universal tool compatibility."),
        ("XFS", "High-performance 64-bit journaling filesystem designed for high concurrency and heavy I/O throughput."),
        ("F2FS", "Flash-Friendly File System designed from the ground up for NAND flash memory, NVMe SSDs, and eMMC drives."),
    ];
    for (title, desc) in fss {
        let b = ui::vbox(2);
        b.append(&ui::strong(title));
        b.append(&ui::dim(desc));
        c.append(&b);
    }

    // 5. Hardware & Network Drivers
    let c = ui::card(content, Some("Hardware & Polling Driver Engine"));
    ui::hint(
        &c,
        "The Archstaller installer kernel runs in pure no_std polling mode without interrupts, supporting modern and vintage hardware alike.",
    );
    let drivers = [
        ("Storage Drivers", "NVMe (PCIe SSDs), AHCI (SATA HDDs/SSDs), legacy ATA (IDE controller mode with PIO/BMDMA), VirtIO Block."),
        ("Modern Network Drivers", "Intel Gigabit (e1000, igb), Realtek Gigabit (r8169/r8168), Qualcomm Atheros (alx), VMware (vmxnet3), VirtIO Net."),
        ("Legacy Network Drivers", "Intel Fast Ethernet PRO/100 (e100 / 8255x series), AMD PCnet32 (Am79C97x Lance), Realtek RTL8139 (10/100)."),
        ("USB Tethering", "Android USB tethering (RNDIS, CDC-NCM), iPhone tethering (Apple MobileDevice multiplexer), ASIX AX88179."),
    ];
    for (title, desc) in drivers {
        let b = ui::vbox(2);
        b.append(&ui::strong(title));
        b.append(&ui::dim(desc));
        c.append(&b);
    }

    // 6. Keyboard Shortcuts
    let c = ui::card(content, Some("Keyboard Shortcuts"));
    let shortcuts = [
        ("Ctrl + N", "New configuration from blank template"),
        ("Ctrl + O", "Open existing TOML configuration file"),
        ("Ctrl + S", "Save current configuration"),
        ("Ctrl + Shift + S", "Save current configuration to a new file"),
        ("Ctrl + P", "Open Command Launcher for quick search"),
        ("Ctrl + B", "Navigate to Build ISO page"),
    ];
    for (key, action) in shortcuts {
        let r = ui::hbox(12);
        let chip = ui::chip(key, "chip-accent");
        chip.set_width_request(140);
        r.append(&chip);
        r.append(&ui::dim(action));
        c.append(&r);
    }
}

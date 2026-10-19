use crate::app::App;
use crate::dialogs;
use crate::ui;
use gtk::glib;
use gtk::prelude::*;
use std::rc::Rc;

pub fn build(app: &Rc<App>, content: &gtk::Box) {
    let cfg = app.st.borrow().model.cfg.clone();
    ui::heading(content, "Disk & Boot", "Target disk selection, bootloader setup, and filesystem formatting.");

    // 1. Bootloader
    let c = ui::card(content, Some("Bootloader"));
    ui::hint(
        &c,
        "The bootloader installed onto the target system at first boot.",
    );
    let loaders = [
        ("grub", "GRUB (BIOS & UEFI universal, default)"),
        ("systemd-boot", "systemd-boot (minimalist UEFI standard)"),
        ("limine", "Limine (modern, fast, BIOS & UEFI)"),
    ];
    let mut group_leader: Option<gtk::CheckButton> = None;
    for (id, desc) in loaders {
        let b = if let Some(ref leader) = group_leader {
            gtk::CheckButton::builder()
                .label(desc)
                .group(leader)
                .build()
        } else {
            let leader = gtk::CheckButton::builder().label(desc).build();
            group_leader = Some(leader.clone());
            leader
        };
        let is_selected = match cfg.bootloader {
            config::Bootloader::Grub => id == "grub",
            config::Bootloader::SystemdBoot => id == "systemd-boot",
            config::Bootloader::Limine => id == "limine",
        };
        b.set_active(is_selected);
        let a = app.clone();
        let target_bl = match id {
            "systemd-boot" => config::Bootloader::SystemdBoot,
            "limine" => config::Bootloader::Limine,
            _ => config::Bootloader::Grub,
        };
        b.connect_toggled(move |btn| {
            if btn.is_active() {
                a.edit(move |m| m.cfg.bootloader = target_bl);
            }
        });
        c.append(&b);
    }

    // 2. Filesystem
    let c = ui::card(content, Some("Filesystem"));
    ui::hint(
        &c,
        "Root filesystem formatting type.",
    );
    let fs_types = [
        (config::FsType::Ext4, "ext4 (classic, robust standard)"),
        (config::FsType::Btrfs, "btrfs (copy-on-write, subvolumes & snapshots)"),
        (config::FsType::Xfs, "xfs (high performance, enterprise scalability)"),
        (config::FsType::F2fs, "f2fs (optimized for flash/NVMe storage)"),
    ];
    let mut fs_group_leader: Option<gtk::CheckButton> = None;
    for (fs, desc) in fs_types {
        let b = if let Some(ref leader) = fs_group_leader {
            gtk::CheckButton::builder()
                .label(desc)
                .group(leader)
                .build()
        } else {
            let leader = gtk::CheckButton::builder().label(desc).build();
            fs_group_leader = Some(leader.clone());
            leader
        };
        b.set_active(cfg.disk.filesystem == fs);
        let a = app.clone();
        b.connect_toggled(move |btn| {
            if btn.is_active() {
                a.edit(move |m| {
                    m.cfg.disk.filesystem = fs;
                    if fs == config::FsType::Btrfs && m.cfg.disk.btrfs.is_none() {
                        m.cfg.disk.btrfs = Some(config::BtrfsConfig::default());
                    }
                });
            }
        });
        c.append(&b);
    }

    // 3. Target disk
    let c = ui::card(content, Some("Which disk"));
    let warn = ui::hint(
        &c,
        "Warning: Every machine that boots this ISO will erase its largest disk automatically.",
    );
    warn.set_visible(cfg.disk.auto_largest);
    let selectors = ui::vbox(8);

    let auto = ui::check_button("Erase and install onto the largest disk, without asking");
    auto.set_active(cfg.disk.auto_largest);
    c.insert_child_after(&auto, c.first_child().as_ref());
    {
        let a = app.clone();
        let warn = warn.clone();
        let selectors = selectors.clone();
        auto.connect_toggled(move |btn| {
            let on = btn.is_active();
            if on && !a.st.borrow().model.cfg.disk.auto_largest {
                // Irreversible for every machine that boots the ISO: ask first, and undo the toggle on "no".
                let a2 = a.clone();
                let btn = btn.clone();
                let (warn, selectors) = (warn.clone(), selectors.clone());
                glib::spawn_future_local(async move {
                    let ok = dialogs::confirm(a2.win.upcast_ref(), "Erase the largest disk without asking?", "With this option the installer wipes and installs onto the largest disk of whatever machine boots this ISO, with no confirmation. Use it only for machines you intend to erase.", "Yes, erase").await;
                    if ok {
                        a2.edit(|m| m.cfg.disk.auto_largest = true);
                        warn.set_visible(true);
                        selectors.set_sensitive(false);
                    } else {
                        btn.set_active(false);
                    }
                });
            } else if !on {
                a.edit(|m| m.cfg.disk.auto_largest = false);
                warn.set_visible(false);
                selectors.set_sensitive(true);
            }
        });
    }

    c.append(&selectors);
    selectors.set_sensitive(!cfg.disk.auto_largest);
    let a = app.clone();
    ui::row(
        &selectors,
        "Serial",
        &ui::entry(&cfg.disk.confirm_serial, move |t| {
            let t = t.to_string();
            a.edit(move |m| m.cfg.disk.confirm_serial = t);
        }),
    );
    let model_row = ui::hbox(8);
    let model_entry = ui::entry(cfg.disk.model.as_deref().unwrap_or(""), {
        let a = app.clone();
        move |t| {
            let t = t.to_string();
            a.edit(move |m| {
                if m.cfg.disk.model.is_some() {
                    m.cfg.disk.model = Some(t);
                }
            });
        }
    });
    model_entry.set_sensitive(cfg.disk.model.is_some());
    let has = ui::check("must also contain", cfg.disk.model.is_some(), {
        let a = app.clone();
        let e = model_entry.clone();
        move |on| {
            let text = e.text().to_string();
            e.set_sensitive(on);
            a.edit(move |m| m.cfg.disk.model = on.then_some(text));
        }
    });
    model_row.append(&has);
    model_row.append(&model_entry);
    ui::row(&selectors, "Model", &model_row);

    // 4. Layout
    let c = ui::card(content, Some("Layout"));
    let a = app.clone();
    let sp = ui::spin(cfg.disk.esp_mib, 64, 8192, move |v| {
        a.edit(move |m| m.cfg.disk.esp_mib = v)
    });
    let r = ui::hbox(6);
    r.append(&sp);
    r.append(&ui::dim("MiB"));
    ui::row(&c, "ESP size", &r);
    ui::hint(
        &c,
        "The FAT32 boot partition, mounted at /boot. The rest of the disk is the root file system.",
    );
}

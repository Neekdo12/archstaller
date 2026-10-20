use crate::app::{App, Page};
use crate::dialogs;
use crate::ui;
use gtk::glib;
use gtk::prelude::*;
use std::rc::Rc;

pub fn build(app: &Rc<App>, content: &gtk::Box) {
    let cfg = app.st.borrow().model.cfg.clone();
    ui::heading(
        content,
        "Disk & bootloader",
        "Target disk selection, bootloader installation, and filesystem layout.",
    );

    // 1. Bootloader Card (at the top of the Disk page)
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
            gtk::CheckButton::builder().label(desc).group(leader).build()
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

    // 2. Which disk / Target disk selection
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
                let a2 = a.clone();
                let btn = btn.clone();
                let (warn, selectors) = (warn.clone(), selectors.clone());
                glib::spawn_future_local(async move {
                    let ok = dialogs::confirm(
                        a2.win.upcast_ref(),
                        "Erase the largest disk without asking?",
                        "With this option the installer wipes and installs onto the largest disk of whatever machine boots this ISO, with no confirmation. Use it only for machines you intend to erase.",
                        "Yes, erase",
                    ).await;
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

    // 3. Layout and Filesystem
    let c = ui::card(content, Some("Filesystem & Partition Layout"));
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

    // Filesystem type selection
    let fs_box = ui::hbox(12);
    let filesystems = [
        (config::FsType::Ext4, "ext4"),
        (config::FsType::Btrfs, "btrfs"),
        (config::FsType::Xfs, "xfs"),
        (config::FsType::F2fs, "f2fs"),
    ];
    let mut fs_leader: Option<gtk::CheckButton> = None;
    for (fs_type, label) in filesystems {
        let b = if let Some(ref leader) = fs_leader {
            gtk::CheckButton::builder().label(label).group(leader).build()
        } else {
            let leader = gtk::CheckButton::builder().label(label).build();
            fs_leader = Some(leader.clone());
            leader
        };
        b.set_active(cfg.disk.filesystem == fs_type);
        let a = app.clone();
        b.connect_toggled(move |btn| {
            if btn.is_active() {
                a.edit(move |m| {
                    m.cfg.disk.filesystem = fs_type;
                    if fs_type == config::FsType::Btrfs && m.cfg.disk.btrfs.is_none() {
                        m.cfg.disk.btrfs = Some(config::BtrfsConfig::default());
                    }
                });
                a.rebuild(Page::Disk);
            }
        });
        fs_box.append(&b);
    }
    ui::row(&c, "Root filesystem", &fs_box);

    // Btrfs options (subvolumes and compression)
    if cfg.disk.filesystem == config::FsType::Btrfs {
        let btrfs_box = ui::vbox(8);
        btrfs_box.set_margin_top(8);

        let btrfs_cfg = cfg.disk.btrfs.clone().unwrap_or_default();
        let a = app.clone();
        ui::row(
            &btrfs_box,
            "Compression",
            &ui::entry(&btrfs_cfg.compress, move |val| {
                let v = val.trim().to_string();
                a.edit(move |m| {
                    if let Some(b) = &mut m.cfg.disk.btrfs {
                        b.compress = v;
                    }
                });
            }),
        );

        btrfs_box.append(&ui::strong("Btrfs Subvolumes"));
        for (idx, subvol) in btrfs_cfg.subvolumes.iter().enumerate() {
            let row = ui::hbox(8);
            row.append(&ui::chip(&subvol.name, "chip-accent"));
            row.append(&ui::dim("mounted at"));
            let m_lbl = ui::strong(&subvol.mount);
            m_lbl.set_hexpand(true);
            row.append(&m_lbl);

            let a = app.clone();
            row.append(&ui::button("Remove", move || {
                a.edit(move |m| {
                    if let Some(b) = &mut m.cfg.disk.btrfs {
                        if idx < b.subvolumes.len() {
                            b.subvolumes.remove(idx);
                        }
                    }
                });
                a.rebuild(Page::Disk);
            }));
            btrfs_box.append(&row);
        }

        // Add subvolume row
        let add_row = ui::hbox(8);
        let name_entry = ui::entry("", |_| {});
        name_entry.set_placeholder_text(Some("Subvolume name (e.g. @data)"));
        let mount_entry = ui::entry("", |_| {});
        mount_entry.set_placeholder_text(Some("Mount path (e.g. /data)"));

        let a = app.clone();
        let ne = name_entry.clone();
        let me = mount_entry.clone();
        let add_btn = ui::button("Add subvolume", move || {
            let name = ne.text().trim().to_string();
            let mount = me.text().trim().to_string();
            if !name.is_empty() && !mount.is_empty() {
                a.edit(move |m| {
                    let b = m.cfg.disk.btrfs.get_or_insert_with(config::BtrfsConfig::default);
                    b.subvolumes.push(config::BtrfsSubvol {
                        name,
                        mount,
                    });
                });
                a.rebuild(Page::Disk);
            }
        });
        add_row.append(&name_entry);
        add_row.append(&mount_entry);
        add_row.append(&add_btn);
        btrfs_box.append(&add_row);

        let a = app.clone();
        let reset_btn = ui::button("Reset to default subvolumes (@, @home, @snapshots, @var_log)", move || {
            a.edit(|m| {
                if let Some(b) = &mut m.cfg.disk.btrfs {
                    b.subvolumes = config::default_btrfs_subvolumes();
                }
            });
            a.rebuild(Page::Disk);
        });
        btrfs_box.append(&reset_btn);

        c.append(&btrfs_box);
    }
}

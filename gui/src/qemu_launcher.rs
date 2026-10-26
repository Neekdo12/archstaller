//! Graphical QEMU E2E Test Launcher Dialog
//!
//! Provides an interactive dialog to configure and run QEMU test environments
//! directly from the Archstaller GUI.

use crate::app::App;
use crate::ui;
use gtk::glib;
use gtk::prelude::*;
use hostcfg::qemu::{DiskController, DisplayBackend, Firmware, NicModel, QemuConfig};
use std::cell::RefCell;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

struct QemuState {
    child: Option<Child>,
    log_buffer: gtk::TextBuffer,
    status_label: gtk::Label,
    launch_btn: gtk::Button,
    stop_btn: gtk::Button,
}

pub fn show_qemu_dialog(app: &Rc<App>) {
    let dialog = gtk::Window::builder()
        .title("Run in QEMU (Graphical E2E Test)")
        .transient_for(&app.win)
        .modal(true)
        .default_width(620)
        .default_height(680)
        .build();

    let (scroll, content) = ui::page_shell();
    dialog.set_child(Some(&scroll));

    ui::heading(
        &content,
        "Run in QEMU",
        "Boot the compiled ISO in a virtual machine to test installation, bootloaders, and UI.",
    );

    let default_iso = app
        .st
        .borrow()
        .media
        .iso_override
        .clone()
        .or_else(|| app.st.borrow().build.iso.as_ref().map(|i| i.0.clone()))
        .unwrap_or_else(|| {
            crate::build::find_root()
                .map(|r| r.join("target/archstaller.iso"))
                .unwrap_or_else(|_| PathBuf::from("target/archstaller.iso"))
        });

    let config = Rc::new(RefCell::new(QemuConfig {
        iso_path: default_iso,
        ..Default::default()
    }));

    // Hardware & Firmware configuration card
    let c = ui::card(&content, Some("Virtual Machine Hardware"));

    // Firmware
    let fw_dd = gtk::DropDown::from_strings(&["UEFI (OVMF)", "Legacy BIOS (SeaBIOS)"]);
    fw_dd.set_selected(0);
    {
        let cfg = config.clone();
        fw_dd.connect_selected_notify(move |dd| {
            cfg.borrow_mut().firmware = match dd.selected() {
                1 => Firmware::Bios,
                _ => Firmware::Uefi,
            };
        });
    }
    ui::row(&c, "Firmware", &fw_dd);

    // Disk controller
    let disk_dd = gtk::DropDown::from_strings(&[
        "VirtIO-Block (Modern Linux)",
        "NVMe (PCIe Express)",
        "AHCI (SATA)",
        "IDE / PIIX3 (Legacy)",
    ]);
    disk_dd.set_selected(0);
    {
        let cfg = config.clone();
        disk_dd.connect_selected_notify(move |dd| {
            cfg.borrow_mut().disk_controller = match dd.selected() {
                1 => DiskController::Nvme,
                2 => DiskController::Ahci,
                3 => DiskController::Ide,
                _ => DiskController::Virtio,
            };
        });
    }
    ui::row(&c, "Disk Controller", &disk_dd);

    // Disk Size
    let disk_size_spin = ui::spin(20, 1, 500, {
        let cfg = config.clone();
        move |sz| cfg.borrow_mut().disk_size_gib = sz
    });
    ui::row(&c, "Disk Size (GiB)", &disk_size_spin);

    // Keep Disk
    let keep_disk_cb = ui::check("Keep virtual disk after QEMU exits", false, {
        let cfg = config.clone();
        move |on| cfg.borrow_mut().keep_disk = on
    });
    c.append(&keep_disk_cb);

    // Network Interface
    let nic_dd = gtk::DropDown::from_strings(&[
        "VirtIO-Net (High Performance)",
        "Intel e1000 (Gigabit)",
        "Intel e1000e (PCIe Gigabit)",
        "Realtek rtl8139 (Fast Ethernet)",
        "Intel e100 (i82559er)",
        "AMD PCnet32 (Am79C970A)",
        "None (Disconnected)",
    ]);
    nic_dd.set_selected(0);
    {
        let cfg = config.clone();
        nic_dd.connect_selected_notify(move |dd| {
            cfg.borrow_mut().nic = match dd.selected() {
                1 => NicModel::E1000,
                2 => NicModel::E1000e,
                3 => NicModel::Rtl8139,
                4 => NicModel::E100,
                5 => NicModel::Pcnet,
                6 => NicModel::None,
                _ => NicModel::Virtio,
            };
        });
    }
    ui::row(&c, "Network Interface", &nic_dd);

    // RAM & CPU Cores
    let ram_spin = ui::spin(4096, 256, 65536, {
        let cfg = config.clone();
        move |ram| cfg.borrow_mut().ram_mib = ram
    });
    ui::row(&c, "Memory (MiB)", &ram_spin);

    let cpu_spin = ui::spin(2, 1, 32, {
        let cfg = config.clone();
        move |cores| cfg.borrow_mut().cpu_cores = cores
    });
    ui::row(&c, "CPU Cores", &cpu_spin);

    // Display & Graphics
    let c = ui::card(&content, Some("Display & Acceleration"));
    let display_dd = gtk::DropDown::from_strings(&[
        "GTK Window (OpenGL accelerated)",
        "SDL Window",
        "VNC Server (:1)",
        "Headless (Serial console only)",
    ]);
    display_dd.set_selected(0);
    {
        let cfg = config.clone();
        display_dd.connect_selected_notify(move |dd| {
            cfg.borrow_mut().display = match dd.selected() {
                1 => DisplayBackend::Sdl,
                2 => DisplayBackend::Vnc,
                3 => DisplayBackend::None,
                _ => DisplayBackend::Gtk,
            };
        });
    }
    ui::row(&c, "Display Backend", &display_dd);

    let kvm_cb = ui::check("Enable KVM Hardware Acceleration", true, {
        let cfg = config.clone();
        move |on| cfg.borrow_mut().enable_kvm = on
    });
    c.append(&kvm_cb);

    // Status and console output card
    let c = ui::card(&content, Some("Execution & Serial Console"));
    let status_label = gtk::Label::builder()
        .label("Ready to launch virtual machine.")
        .xalign(0.0)
        .build();
    c.append(&status_label);

    let log_buffer = gtk::TextBuffer::new(None);
    let log_view = gtk::TextView::builder()
        .buffer(&log_buffer)
        .editable(false)
        .cursor_visible(false)
        .monospace(true)
        .wrap_mode(gtk::WrapMode::Char)
        .build();

    let log_scroll = gtk::ScrolledWindow::builder()
        .child(&log_view)
        .min_content_height(160)
        .max_content_height(240)
        .hexpand(true)
        .build();
    c.append(&log_scroll);

    // Buttons
    let action_box = ui::hbox(8);
    let launch_btn = ui::primary("Launch QEMU", || {});
    let stop_btn = gtk::Button::with_label("Stop VM");
    stop_btn.add_css_class("destructive-action");
    stop_btn.set_sensitive(false);

    let close_btn = ui::button("Close", {
        let d = dialog.clone();
        move || d.close()
    });

    action_box.append(&launch_btn);
    action_box.append(&stop_btn);
    action_box.append(&close_btn);
    content.append(&action_box);

    let state = Rc::new(RefCell::new(QemuState {
        child: None,
        log_buffer: log_buffer.clone(),
        status_label: status_label.clone(),
        launch_btn: launch_btn.clone(),
        stop_btn: stop_btn.clone(),
    }));

    // Launch action
    {
        let state = state.clone();
        let cfg_rc = config;
        launch_btn.connect_clicked(move |_| {
            let cfg = cfg_rc.borrow().clone();
            if !cfg.iso_path.exists() {
                state.borrow().status_label.set_label(&format!(
                    "ISO file not found at: {}",
                    cfg.iso_path.display()
                ));
                return;
            }

            if let Err(e) = cfg.create_disk_image() {
                state
                    .borrow()
                    .status_label
                    .set_label(&format!("Failed to provision disk: {e}"));
                return;
            }

            match cfg.build_command() {
                Ok(mut cmd) => {
                    cmd.stdout(Stdio::piped());
                    cmd.stderr(Stdio::piped());

                    match cmd.spawn() {
                        Ok(mut child) => {
                            let pid = child.id();
                            state.borrow().status_label.set_label(&format!(
                                "VM is running (PID: {pid}). Output streaming..."
                            ));
                            state.borrow().launch_btn.set_sensitive(false);
                            state.borrow().stop_btn.set_sensitive(true);

                            let stdout = child.stdout.take();
                            let stderr = child.stderr.take();
                            state.borrow_mut().child = Some(child);

                            let (tx, rx) = mpsc::channel::<String>();

                            if let Some(out) = stdout {
                                let tx_out = tx.clone();
                                std::thread::spawn(move || {
                                    let reader = BufReader::new(out);
                                    for line in reader.lines().map_while(Result::ok) {
                                        if tx_out.send(format!("{line}\n")).is_err() {
                                            break;
                                        }
                                    }
                                });
                            }

                            if let Some(err) = stderr {
                                let tx_err = tx;
                                std::thread::spawn(move || {
                                    let reader = BufReader::new(err);
                                    for line in reader.lines().map_while(Result::ok) {
                                        if tx_err.send(format!("[STDERR] {line}\n")).is_err() {
                                            break;
                                        }
                                    }
                                });
                            }

                            let buf = state.borrow().log_buffer.clone();
                            glib::timeout_add_local(Duration::from_millis(50), move || {
                                while let Ok(line) = rx.try_recv() {
                                    let mut end = buf.end_iter();
                                    buf.insert(&mut end, &line);
                                }
                                glib::ControlFlow::Continue
                            });
                        }
                        Err(e) => {
                            state
                                .borrow()
                                .status_label
                                .set_label(&format!("Failed to spawn QEMU: {e}"));
                        }
                    }
                }
                Err(e) => {
                    state
                        .borrow()
                        .status_label
                        .set_label(&format!("Configuration error: {e}"));
                }
            }
        });
    }

    // Stop action
    {
        let state = state.clone();
        stop_btn.connect_clicked(move |_| {
            if let Some(mut child) = state.borrow_mut().child.take() {
                drop(child.kill());
                drop(child.wait());
                state
                    .borrow()
                    .status_label
                    .set_label("VM stopped by user.");
                state.borrow().launch_btn.set_sensitive(true);
                state.borrow().stop_btn.set_sensitive(false);
            }
        });
    }

    // Cleanup on dialog close
    dialog.connect_close_request(move |_| {
        if let Some(mut child) = state.borrow_mut().child.take() {
            drop(child.kill());
            drop(child.wait());
        }
        glib::Propagation::Proceed
    });

    dialog.present();
}

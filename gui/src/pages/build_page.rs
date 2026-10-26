use crate::app::{App, Page};
use crate::dialogs;
use crate::ui;
use gtk::glib;
use gtk::prelude::*;
use hostcfg::download_tester::DownloadTesterConfig;
use hostcfg::host::{DriverClass, LogVerbosity, DRIVERS};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

pub fn build(app: &Rc<App>, content: &gtk::Box) {
    let (host, tethering) = {
        let st = app.st.borrow();
        (st.model.host.clone(), st.model.host.build.tethering)
    };
    ui::heading(
        content,
        "Build & drivers",
        "How the installer ISO itself is built. These settings are not part of the installed system.",
    );

    let c = ui::card(content, Some("Build profile"));
    let current = host
        .build
        .profile
        .clone()
        .unwrap_or_else(|| "super-small".into());
    let small = ui::check_button("super-small");
    let large = ui::check_button("large");
    large.set_group(Some(&small));
    small.set_active(current != "large");
    large.set_active(current == "large");
    for (btn, name) in [(&small, "super-small"), (&large, "large")] {
        let a = app.clone();
        btn.connect_toggled(move |b| {
            if b.is_active() {
                a.edit(move |m| m.set_profile(name));
            }
        });
    }
    c.append(&small);
    ui::hint(
        &c,
        "Size-optimized installer (the default), about 0.7 MiB. A crash shows no panic message.",
    );
    c.append(&large);
    ui::hint(
        &c,
        "The regular release build, about 0.8 MiB, with panic messages kept for debugging.",
    );

    let c = ui::card(content, Some("USB tethering"));
    let a = app.clone();
    c.append(&ui::check(
        "Include USB tethering (iPhone, Android, USB Ethernet)",
        tethering,
        move |on| a.edit(move |m| m.host.build.tethering = on),
    ));
    ui::hint(
        &c,
        "Adds about 100 KiB. Lets the installer use a phone's hotspot when no wired network has a link.",
    );

    let c = ui::card(content, Some("Installer drivers"));
    ui::hint(
        &c,
        "Drivers of the installer itself, not packages for the installed system. Leave them all on unless you know the hardware.",
    );
    let all = host.installer_drivers.is_none();
    let a = app.clone();
    c.append(&ui::check("All drivers", all, move |on| {
        a.edit(move |m| {
            m.host.installer_drivers = if on {
                None
            } else {
                Some(DRIVERS.iter().map(|d| d.id.to_string()).collect())
            }
        });
        a.rebuild(Page::Build);
    }));
    if let Some(list) = host.installer_drivers.clone() {
        for (class, title) in [
            (DriverClass::Storage, "Storage"),
            (DriverClass::Network, "Network"),
        ] {
            c.append(&ui::strong(title));
            for d in DRIVERS.iter().filter(|d| d.class == class) {
                let r = ui::hbox(12);
                let id = d.id;
                let a = app.clone();
                let on = list.iter().any(|i| i == id);
                let cb = ui::check(id, on, move |on| {
                    a.edit(move |m| {
                        let l = m.host.installer_drivers.get_or_insert_with(Vec::new);
                        if on {
                            if !l.iter().any(|i| i == id) {
                                l.push(id.to_string());
                            }
                        } else {
                            l.retain(|i| i != id);
                        }
                    });
                });
                cb.set_width_request(130);
                r.append(&cb);
                let desc = gtk::Label::builder()
                    .label(d.description)
                    .xalign(0.0)
                    .hexpand(true)
                    .wrap(true)
                    .selectable(true)
                    .build();
                r.append(&desc);
                r.append(&ui::dim(d.status));
                c.append(&r);
            }
        }
    }

    let c = ui::card(content, Some("Hardware test mode"));
    let a = app.clone();
    let dry_run = app.st.borrow().model.cfg.dry_run;
    c.append(&ui::check(
        "Boot in hardware test mode (read-only, no disk writes)",
        dry_run,
        move |on| {
            a.edit(move |m| m.cfg.dry_run = on);
        },
    ));
    ui::hint(
        &c,
        "Probes hardware, reads disks, tests network connectivity and DHCP, and verifies package resolution without modifying disks. Disks do not require confirmation when dry-run is enabled.",
    );

    // Feature 1: In-RAM Package Download Tester
    let c = ui::card(content, Some("In-RAM Package Download Tester"));
    ui::hint(
        &c,
        "Test network mirrors, DNS resolution, and package download verification directly in RAM without modifying disks.",
    );

    let test_cfg = Rc::new(RefCell::new(DownloadTesterConfig {
        mirror: app
            .st
            .borrow()
            .model
            .cfg
            .mirrors
            .first()
            .cloned()
            .unwrap_or_else(|| "https://geo.mirror.pkgbuild.com/$repo/os/$arch".to_string()),
        ..Default::default()
    }));

    let dns_cb = ui::check("Verify DNS resolution and ping latency", true, {
        let cfg = test_cfg.clone();
        move |on| cfg.borrow_mut().test_dns_and_latency = on
    });
    c.append(&dns_cb);

    let sync_cb = ui::check("Synchronize pacman databases into memory", true, {
        let cfg = test_cfg.clone();
        move |on| cfg.borrow_mut().sync_databases = on
    });
    c.append(&sync_cb);

    let sample_cb = ui::check("Download sample packages & verify signatures", true, {
        let cfg = test_cfg.clone();
        move |on| {
            cfg.borrow_mut().verify_signatures = on;
            cfg.borrow_mut().sample_packages = if on {
                vec!["pacman".to_string()]
            } else {
                Vec::new()
            };
        }
    });
    c.append(&sample_cb);

    let mem_spin = ui::spin(512, 16, 4096, {
        let cfg = test_cfg.clone();
        move |val| cfg.borrow_mut().memory_budget_bytes = (val as u64) * 1024 * 1024
    });
    ui::row(&c, "Simulated memory budget (MiB)", &mem_spin);

    let run_tester_btn = ui::primary("Run Download Diagnostics…", {
        let a = app.clone();
        let cfg = test_cfg;
        move || {
            show_download_tester_dialog(a.win.upcast_ref(), cfg.borrow().clone());
        }
    });
    c.append(&run_tester_btn);

    // Feature 8: Logging & diagnostics
    let c = ui::card(content, Some("Logging & diagnostics"));
    ui::hint(
        &c,
        "The installer writes kernel and progress logs to a 128 KiB circular in-RAM ring buffer. On failure or when booting from Ventoy, the full log is saved to archstaller-log.txt.",
    );

    // Kernel trace facilities
    let trace_box = ui::hbox(12);
    let trace = host.trace;
    let net_trace = ui::check("Network", trace.network, {
        let a = app.clone();
        move |on| {
            a.edit(move |m| {
                let mut t = m.host.trace;
                t.network = on;
                m.set_trace(t);
            });
        }
    });
    let usb_trace = ui::check("USB", trace.usb, {
        let a = app.clone();
        move |on| {
            a.edit(move |m| {
                let mut t = m.host.trace;
                t.usb = on;
                m.set_trace(t);
            });
        }
    });
    let storage_trace = ui::check("Storage", trace.storage, {
        let a = app.clone();
        move |on| {
            a.edit(move |m| {
                let mut t = m.host.trace;
                t.storage = on;
                m.set_trace(t);
            });
        }
    });
    let timer_trace = ui::check("Timer", trace.timer, {
        let a = app.clone();
        move |on| {
            a.edit(move |m| {
                let mut t = m.host.trace;
                t.timer = on;
                m.set_trace(t);
            });
        }
    });
    trace_box.append(&net_trace);
    trace_box.append(&usb_trace);
    trace_box.append(&storage_trace);
    trace_box.append(&timer_trace);
    ui::row(&c, "Kernel trace facilities", &trace_box);
    ui::hint(
        &c,
        "Enables low-level packet and queue logging in the installer kernel.",
    );

    // Log verbosity dropdown
    let verbosity_strings = ["Error", "Warn", "Info", "Debug", "Trace"];
    let verb_dd = gtk::DropDown::from_strings(&verbosity_strings);
    let cur_idx = match host.log_verbosity {
        LogVerbosity::Error => 0,
        LogVerbosity::Warn => 1,
        LogVerbosity::Info => 2,
        LogVerbosity::Debug => 3,
        LogVerbosity::Trace => 4,
    };
    verb_dd.set_selected(cur_idx);
    {
        let a = app.clone();
        verb_dd.connect_selected_notify(move |dd| {
            let v = match dd.selected() {
                0 => LogVerbosity::Error,
                1 => LogVerbosity::Warn,
                2 => LogVerbosity::Info,
                3 => LogVerbosity::Debug,
                _ => LogVerbosity::Trace,
            };
            a.edit(move |m| m.set_log_verbosity(v));
        });
    }
    ui::row(&c, "Installer log verbosity", &verb_dd);

    // Import and analyze installer log button
    let import_btn = ui::button("Import & Analyze Installer Log…", {
        let win = app.win.clone();
        move || {
            let w = win.clone();
            glib::spawn_future_local(async move {
                if let Some(path) =
                    dialogs::open_file(w.upcast_ref(), "Open archstaller-log.txt", Some(("Log file", "*.txt"))).await
                {
                    if let Ok(text) = std::fs::read_to_string(&path) {
                        show_diagnostics_dialog(w.upcast_ref(), &text);
                    }
                }
            });
        }
    });
    c.append(&import_btn);
}

fn show_diagnostics_dialog(parent: &gtk::Window, log_text: &str) {
    let dialog = gtk::Window::builder()
        .title("Installer Log Analysis & Diagnostics")
        .transient_for(parent)
        .modal(true)
        .default_width(700)
        .default_height(550)
        .build();

    let (scroll, content) = ui::page_shell();
    dialog.set_child(Some(&scroll));

    ui::heading(
        &content,
        "Log Diagnostics & Remediation",
        "Automated failure analysis and actionable recommendations from kernel logs.",
    );

    let entries = hostcfg::diagnostics::parse_log(log_text);
    let diagnostics = hostcfg::diagnostics::analyze_log(&entries);

    if !diagnostics.is_empty() {
        let c = ui::card(&content, Some("Detected Issues & Remediation"));
        for diag in diagnostics {
            let kind = match diag.category {
                hostcfg::diagnostics::DiagnosticCategory::KernelPanic => ui::Kind::Bad,
                _ => ui::Kind::Warn,
            };
            ui::note(
                &c,
                kind,
                &format!("{:?}: {}", diag.category, diag.summary),
            );
            let action_str = match &diag.action {
                Some(hostcfg::diagnostics::RemediationAction::SwitchMirror(m)) => {
                    format!("Switch to mirror: {m}")
                }
                Some(hostcfg::diagnostics::RemediationAction::EnableAutoLargest) => {
                    "Enable auto-largest disk in Disk settings".to_string()
                }
                Some(hostcfg::diagnostics::RemediationAction::EnableTethering) => {
                    "Enable USB tethering in Build & drivers".to_string()
                }
                Some(hostcfg::diagnostics::RemediationAction::RetryWithDebug) => {
                    "Enable debug/trace logging and retry".to_string()
                }
                None => diag.remediation.clone(),
            };
            ui::hint(&c, &format!("Recommended action: {action_str}"));
        }
    } else {
        let c = ui::card(&content, Some("Health Check"));
        ui::note(
            &c,
            ui::Kind::Ok,
            "No critical installer failures detected in the log.",
        );
    }

    let c = ui::card(
        &content,
        Some(&format!("Parsed Entries ({} lines)", entries.len())),
    );
    let log_view = gtk::TextView::builder()
        .editable(false)
        .monospace(true)
        .wrap_mode(gtk::WrapMode::Word)
        .build();
    let buf = log_view.buffer();
    let mut display_text = String::new();
    for entry in entries.iter().take(250) {
        display_text.push_str(&format!(
            "[{:?}] [{:?}] {}\n",
            entry.level,
            entry.subsystem,
            entry.message
        ));
    }
    buf.set_text(&display_text);

    let scroll_log = gtk::ScrolledWindow::builder()
        .child(&log_view)
        .min_content_height(200)
        .max_content_height(350)
        .build();
    c.append(&scroll_log);

    let close_box = ui::hbox(8);
    let close_btn = ui::primary("Close", {
        let d = dialog.clone();
        move || d.close()
    });
    close_box.append(&close_btn);
    content.append(&close_box);

    dialog.present();
}

fn show_download_tester_dialog(parent: &gtk::Window, config: DownloadTesterConfig) {
    let dialog = gtk::Window::builder()
        .title("In-RAM Download Diagnostics")
        .transient_for(parent)
        .modal(true)
        .default_width(620)
        .default_height(480)
        .build();

    let (scroll, content) = ui::page_shell();
    dialog.set_child(Some(&scroll));

    ui::heading(
        &content,
        "Package Download & Network Tester",
        "Benchmarking latency, sync databases, and package signatures in RAM.",
    );

    let c = ui::card(&content, Some("Test Progress"));
    let status_label = gtk::Label::builder()
        .label("Running diagnostics...")
        .xalign(0.0)
        .build();
    c.append(&status_label);

    let spinner = gtk::Spinner::builder().spinning(true).build();
    c.append(&spinner);

    let log_view = gtk::TextView::builder()
        .editable(false)
        .monospace(true)
        .wrap_mode(gtk::WrapMode::Char)
        .build();
    let log_buf = log_view.buffer();
    let scroll_view = gtk::ScrolledWindow::builder()
        .child(&log_view)
        .min_content_height(200)
        .build();
    c.append(&scroll_view);

    let close_btn = ui::button("Close", {
        let d = dialog.clone();
        move || d.close()
    });
    content.append(&close_btn);

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let res = hostcfg::download_tester::run_test_with_transport(
            &config,
            &hostcfg::download_tester::HttpTransport,
            None,
        )
        .map_err(|e| e.to_string());
        drop(tx.send(res));
    });

    let status = status_label.clone();
    let buf = log_buf.clone();
    let spin = spinner.clone();

    glib::timeout_add_local(Duration::from_millis(100), move || {
        if let Ok(res) = rx.try_recv() {
            spin.set_spinning(false);
            match res {
                Ok(report) => {
                    let summary = if report.overall_success {
                        format!(
                            "Diagnostics PASS: synced {} DBs, {} packages tested.",
                            report.databases_synced.len(),
                            report.packages.len()
                        )
                    } else if report.memory_budget_exceeded {
                        "Diagnostics FAIL: simulated memory budget exceeded!".to_string()
                    } else {
                        "Diagnostics completed with errors/warnings.".to_string()
                    };
                    status.set_label(&summary);
                    let text = report.logs.join("\n");
                    buf.set_text(&text);
                }
                Err(e) => {
                    status.set_label(&format!("Test run failed: {e}"));
                }
            }
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });

    dialog.present();
}

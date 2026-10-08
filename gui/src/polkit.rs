//! Raw flashing opens the drive through UDisks2, which asks polkit for the user's password. A polkit
//! authentication agent must be running in the session to show that prompt. Full desktops have one; bare
//! window managers often do not, so the app starts one it finds while it runs and stops it on exit.
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

/// Desktops that run their own agent.
const DESKTOPS: &[&str] = &["GNOME", "KDE", "X-Cinnamon", "Cinnamon", "COSMIC", "Budgie", "Unity", "Pantheon", "MATE", "XFCE", "LXQt", "LXDE", "Deepin"];

/// Process names (the kernel keeps 15 characters) of known agents.
const RUNNING: &[&str] = &["polkit-kde-auth", "polkit-gnome-au", "polkit-mate-aut", "lxqt-policykit", "xfce-polkit", "hyprpolkitagent", "lxpolkit", "polkit-agent", "gnome-shell", "plasmashell", "cinnamon", "cosmic-osd", "budgie-polkit"];

/// Agent programs to start, first match wins.
const AGENTS: &[&str] = &[
    "/usr/lib/polkit-kde-authentication-agent-1",
    "/usr/libexec/polkit-kde-authentication-agent-1",
    "/usr/lib/x86_64-linux-gnu/libexec/polkit-kde-authentication-agent-1",
    "/usr/lib/polkit-gnome/polkit-gnome-authentication-agent-1",
    "/usr/libexec/polkit-gnome-authentication-agent-1",
    "/usr/lib/polkit-gnome-authentication-agent-1",
    "/usr/lib/mate-polkit/polkit-mate-authentication-agent-1",
    "/usr/libexec/polkit-mate-authentication-agent-1",
    "/usr/lib/xfce-polkit/xfce-polkit",
    "/usr/bin/lxqt-policykit-agent",
    "/usr/bin/lxpolkit",
    "/usr/bin/hyprpolkitagent",
    "/usr/lib/hyprpolkitagent/hyprpolkitagent",
    "/usr/libexec/hyprpolkitagent",
];

static STARTED: Mutex<Option<Child>> = Mutex::new(None);

fn desktop_has_agent() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.split(':').any(|p| DESKTOPS.iter().any(|k| p.eq_ignore_ascii_case(k))))
}

/// Whether a process with one of the agents' names is running.
fn agent_running() -> bool {
    std::fs::read_dir("/proc").into_iter().flatten().flatten().any(|e| {
        let n = e.file_name();
        n.to_string_lossy().bytes().all(|b| b.is_ascii_digit()) && std::fs::read_to_string(e.path().join("comm")).is_ok_and(|c| RUNNING.contains(&c.trim()))
    })
}

/// Starts an authentication agent unless the session already has one. Quiet when none is installed: the
/// flash then reports the missing agent itself.
pub fn start() {
    let mut slot = STARTED.lock().unwrap();
    if slot.is_some() || desktop_has_agent() || agent_running() {
        return;
    }
    let Some(path) = AGENTS.iter().find(|p| std::path::Path::new(p).is_file()) else { return };
    let mut cmd = Command::new(path);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // SAFETY: only `prctl`, which is async-signal-safe, runs between fork and exec. The agent then dies with
    // the app even when the app is killed or crashes.
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    if let Ok(child) = cmd.spawn() {
        *slot = Some(child);
    }
}

/// Stops the agent this app started (not one that was already running).
pub fn stop() {
    if let Some(mut c) = STARTED.lock().unwrap().take() {
        let _ = c.kill();
        let _ = c.wait();
    }
}

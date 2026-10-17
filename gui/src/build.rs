//! Running `cargo xtask build` for the saved config and following its progress events.
use hostcfg::progress::Event;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};

pub enum Msg {
    Event(Event),
    Log(String),
    /// The process ended (`true`: exit status 0).
    Exit(bool),
}

pub struct Build {
    pub rx: Receiver<Msg>,
    child: Arc<Mutex<Child>>,
    pub workdir: PathBuf,
    pub log_path: PathBuf,
}

/// The source tree the ISO is built from: `ARCHSTALLER_ROOT`, else the checkout the executable was built in,
/// else the tree the Arch package installs beside it (`/usr/bin/archstaller-gui` -> `/usr/share/archstaller`),
/// else the checkout around the current directory.
pub fn find_root() -> Result<PathBuf, String> {
    if let Ok(r) = std::env::var("ARCHSTALLER_ROOT") {
        return Ok(PathBuf::from(r));
    }
    search_root(std::env::current_exe().ok().as_deref(), std::env::current_dir().ok().as_deref())
        .ok_or_else(|| "cannot find the archstaller source tree (install the package, run from inside a checkout or set ARCHSTALLER_ROOT)".into())
}

/// A source tree holds `rust-toolchain.toml` and `xtask/`.
fn is_root(p: &Path) -> bool {
    p.join("rust-toolchain.toml").is_file() && p.join("xtask").is_dir()
}

/// `find_root` without the environment. An installed executable takes its own tree before a checkout the
/// current directory happens to be in.
fn search_root(exe: Option<&Path>, cwd: Option<&Path>) -> Option<PathBuf> {
    let checkout = |start: &Path| {
        start
            .ancestors()
            .find(|p| is_root(p))
            .map(Path::to_path_buf)
    };
    let installed = exe
        .and_then(|e| Some(e.parent()?.parent()?.join("share/archstaller")))
        .filter(|p| is_root(p));
    exe.and_then(checkout)
        .or(installed)
        .or_else(|| cwd.and_then(checkout))
}

/// Cargo's build directory for `root`: `CARGO_TARGET_DIR`, else `target/` of a checkout we may write to, else
/// `~/.cache/archstaller/target`, where the `archstaller` command builds too. Nothing is written under `/usr`
/// (the tree the package installs), not even by root.
pub fn target_dir(root: &Path) -> PathBuf {
    if let Some(dir) = std::env::var_os("CARGO_TARGET_DIR") {
        return root.join(dir);
    }
    use std::os::unix::ffi::OsStrExt;
    let writable = || {
        std::ffi::CString::new(root.as_os_str().as_bytes()).is_ok_and(|p| {
            // SAFETY: `p` is a valid null-terminated C string created from root path bytes.
            unsafe { libc::access(p.as_ptr(), libc::W_OK) == 0 }
        })
    };
    if !root.starts_with("/usr") && writable() {
        root.join("target")
    } else {
        dirs::cache_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("archstaller/target")
    }
}

impl Build {
    /// Starts a build of `config` into a per-build workspace under `<target dir>/gui-builds/`.
    pub fn start(root: &Path, config: &Path, id: &str) -> Result<Build, String> {
        Self::start_cmd(root, config, id, "build", "archstaller.iso")
    }

    /// Starts a build of the 1.44 MB floppy image for `config`.
    pub fn start_floppy(root: &Path, config: &Path, id: &str) -> Result<Build, String> {
        Self::start_cmd(root, config, id, "floppy", "archstaller-floppy.img")
    }

    /// Starts a build of the Raspberry Pi AArch64 image for `config`.
    pub fn start_rpi(root: &Path, config: &Path, id: &str) -> Result<Build, String> {
        Self::start_cmd(root, config, id, "rpi-build", "archstaller-rpi.img")
    }

    fn start_cmd(
        root: &Path,
        config: &Path,
        id: &str,
        cmd: &str,
        out_file: &str,
    ) -> Result<Build, String> {
        let target = target_dir(root);
        let base = target.join("gui-builds");
        let workdir = base.join(id);
        std::fs::create_dir_all(&workdir).map_err(|e| e.to_string())?;
        let log_path = base.join(format!("{id}.log"));
        use std::os::unix::process::CommandExt;
        let mut child = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
            // Its own process group, so that cancelling stops xtask and the builds it runs, not only cargo.
            .process_group(0)
            .current_dir(root)
            // cargo and xtask must agree on where the builds land.
            .env("CARGO_TARGET_DIR", &target)
            .args(["xtask", cmd, "--config"])
            .arg(config)
            .arg("--out")
            .arg(workdir.join(out_file))
            .arg("--workdir")
            .arg(&workdir)
            .args(["--progress", "json"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot run cargo: {e}"))?;
        let (tx, rx) = channel();
        let out = child.stdout.take().ok_or("cannot capture stdout")?;
        let err = child.stderr.take().ok_or("cannot capture stderr")?;
        let (t1, t2) = (tx.clone(), tx.clone());
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                drop(t1.send(match Event::parse(&line) {
                    Some(e) => Msg::Event(e),
                    None => Msg::Log(line),
                }));
            }
        });
        std::thread::spawn(move || {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                drop(t2.send(Msg::Log(line)));
            }
        });
        let child = Arc::new(Mutex::new(child));
        let waiter = child.clone();
        std::thread::spawn(move || loop {
            let Ok(mut child) = waiter.lock() else {
                break;
            };
            let status = child.try_wait();
            drop(child);
            match status {
                Ok(Some(s)) => {
                    // Let the readers drain what is left in the pipes first.
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    drop(tx.send(Msg::Exit(s.success())));
                    break;
                }
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(100)),
                Err(_) => {
                    drop(tx.send(Msg::Exit(false)));
                    break;
                }
            }
        });
        Ok(Build {
            rx,
            child,
            workdir,
            log_path,
        })
    }

    /// Stops the build: cargo, the xtask it runs and everything that starts (compilers, xorriso). Only this
    /// build's workspace is removed; the log stays.
    pub fn cancel(&self) {
        let Ok(mut child) = self.child.lock() else {
            return;
        };
        if child.try_wait().is_ok_and(|s| s.is_none()) {
            // The group's id is cargo's pid (process_group(0)); killing only cargo left xtask running.
            // SAFETY: child.id() is a valid pid and casting to negative i32 targets the process group created with process_group(0).
            unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
        }
        drop(child.kill());
        drop(std::fs::remove_dir_all(&self.workdir));
    }
}

/// A finished, usable ISO: the file exists and is readable at its recorded size.
pub fn usable(path: &Path, size: u64) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.len() == size) && std::fs::File::open(path).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usable_checks_size_and_existence() {
        let p = std::env::temp_dir().join(format!("archstaller-usable-{}", std::process::id()));
        std::fs::write(&p, b"12345").unwrap();
        assert!(usable(&p, 5));
        assert!(!usable(&p, 6));
        drop(std::fs::remove_file(&p));
        assert!(!usable(&p, 5));
    }

    #[test]
    fn finds_this_checkout() {
        let root = find_root().unwrap();
        assert!(root.join("rust-toolchain.toml").is_file());
    }

    #[test]
    fn an_installed_executable_finds_the_tree_beside_it() {
        let base =
            std::env::temp_dir().join(format!("archstaller-installed-{}", std::process::id()));
        let tree = base.join("usr/share/archstaller");
        std::fs::create_dir_all(tree.join("xtask")).unwrap();
        std::fs::write(tree.join("rust-toolchain.toml"), "").unwrap();
        let exe = base.join("usr/bin/archstaller-gui");
        let checkout = find_root().unwrap();
        assert_eq!(search_root(Some(&exe), None), Some(tree.clone()));
        // Started inside a checkout, the installed executable still uses its own tree.
        assert_eq!(
            search_root(Some(&exe), Some(&checkout.join("gui"))),
            Some(tree)
        );
        // A copied executable with no tree beside it falls back to the current directory.
        assert_eq!(
            search_root(
                Some(&base.join("bin/archstaller-gui")),
                Some(&checkout.join("gui"))
            ),
            Some(checkout)
        );
        drop(std::fs::remove_dir_all(&base));
    }
}

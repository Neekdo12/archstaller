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

/// The repository root: `ARCHSTALER_ROOT`, else the nearest parent of the current directory or the
/// executable that holds `rust-toolchain.toml` and `xtask/`.
pub fn find_root() -> Result<PathBuf, String> {
    if let Ok(r) = std::env::var("ARCHSTALER_ROOT") {
        return Ok(PathBuf::from(r));
    }
    let starts = [std::env::current_dir().ok(), std::env::current_exe().ok()];
    for s in starts.into_iter().flatten() {
        let mut p: &Path = &s;
        loop {
            if p.join("rust-toolchain.toml").is_file() && p.join("xtask").is_dir() {
                return Ok(p.to_path_buf());
            }
            match p.parent() {
                Some(q) => p = q,
                None => break,
            }
        }
    }
    Err("cannot find the archstaler checkout (run from inside it or set ARCHSTALER_ROOT)".into())
}

impl Build {
    /// Starts a build of `config` into a per-build workspace under `<root>/target/gui-builds/`.
    pub fn start(root: &Path, config: &Path, id: &str) -> Result<Build, String> {
        let base = root.join("target/gui-builds");
        let workdir = base.join(id);
        std::fs::create_dir_all(&workdir).map_err(|e| e.to_string())?;
        let log_path = base.join(format!("{id}.log"));
        let mut child = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
            .current_dir(root)
            .args(["xtask", "build", "--config"])
            .arg(config)
            .arg("--out")
            .arg(workdir.join("archstaler.iso"))
            .arg("--workdir")
            .arg(&workdir)
            .args(["--progress", "json"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot run cargo: {e}"))?;
        let (tx, rx) = channel();
        let out = child.stdout.take().unwrap();
        let err = child.stderr.take().unwrap();
        let (t1, t2) = (tx.clone(), tx.clone());
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                let _ = t1.send(match Event::parse(&line) {
                    Some(e) => Msg::Event(e),
                    None => Msg::Log(line),
                });
            }
        });
        std::thread::spawn(move || {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                let _ = t2.send(Msg::Log(line));
            }
        });
        let child = Arc::new(Mutex::new(child));
        let waiter = child.clone();
        std::thread::spawn(move || loop {
            let status = waiter.lock().unwrap().try_wait();
            match status {
                Ok(Some(s)) => {
                    // Let the readers drain what is left in the pipes first.
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    let _ = tx.send(Msg::Exit(s.success()));
                    break;
                }
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(100)),
                Err(_) => {
                    let _ = tx.send(Msg::Exit(false));
                    break;
                }
            }
        });
        Ok(Build { rx, child, workdir, log_path })
    }

    /// Stops the build. Only this build's workspace is removed; the log stays.
    pub fn cancel(&self) {
        let _ = self.child.lock().unwrap().kill();
        let _ = std::fs::remove_dir_all(&self.workdir);
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
        let p = std::env::temp_dir().join(format!("archstaler-usable-{}", std::process::id()));
        std::fs::write(&p, b"12345").unwrap();
        assert!(usable(&p, 5));
        assert!(!usable(&p, 6));
        let _ = std::fs::remove_file(&p);
        assert!(!usable(&p, 5));
    }

    #[test]
    fn finds_this_checkout() {
        let root = find_root().unwrap();
        assert!(root.join("rust-toolchain.toml").is_file());
    }
}

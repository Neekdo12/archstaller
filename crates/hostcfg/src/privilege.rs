//! Helper functions to read system files or list directories that may require root/sudo privileges.

use std::fs::{self, File, Permissions};
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::{FromRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

/// A collection of files retrieved under root/sudo elevation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrivilegedSnapshot {
    /// Contents of `/etc/shadow` if successfully retrieved.
    pub shadow: Option<String>,
    /// Discovered system overlay files: `(relative_path, mode, content)`.
    pub files: Vec<(String, u32, Vec<u8>)>,
}

/// Executes a command in a pseudoterminal (PTY), establishing a controlling terminal session,
/// interacting with prompts (e.g. su password prompts) and waiting up to `timeout_secs`.
pub fn run_command_in_pty(
    cmd: &str,
    args: &[&str],
    pw: Option<&str>,
    timeout_secs: u64,
) -> Result<i32, String> {
    let mut master: RawFd = 0;
    let mut slave: RawFd = 0;

    // SAFETY: openpty takes pointers to master/slave RawFd and NULL for optional term/winsize structs.
    let res = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if res != 0 {
        return Err("failed to allocate pty".to_string());
    }

    // SAFETY: slave is a valid open file descriptor returned by openpty.
    let slave_file = unsafe { File::from_raw_fd(slave) };
    let slave_in = match slave_file.try_clone() {
        Ok(f) => Stdio::from(f),
        Err(_) => {
            // SAFETY: master is an open descriptor allocated by openpty.
            unsafe { libc::close(master) };
            return Err("cannot clone slave pty fd".to_string());
        }
    };
    let slave_out = match slave_file.try_clone() {
        Ok(f) => Stdio::from(f),
        Err(_) => {
            // SAFETY: master is an open descriptor allocated by openpty.
            unsafe { libc::close(master) };
            return Err("cannot clone slave pty fd".to_string());
        }
    };
    let slave_err = Stdio::from(slave_file);

    let mut child_cmd = Command::new(cmd);
    child_cmd
        .args(args)
        .stdin(slave_in)
        .stdout(slave_out)
        .stderr(slave_err);

    let pre_exec_hook = move || {
        // SAFETY: valid syscall in child process after fork.
        unsafe { libc::setsid() };
        // SAFETY: slave is a valid open file descriptor.
        unsafe { libc::ioctl(slave, libc::TIOCSCTTY, 0) };
        Ok(())
    };
    // SAFETY: pre_exec runs in child process after fork before exec.
    unsafe { child_cmd.pre_exec(pre_exec_hook) };

    let mut child = child_cmd.spawn().map_err(|e| {
        // SAFETY: master is an open descriptor allocated by openpty.
        unsafe { libc::close(master) };
        format!("cannot run {cmd}: {e}")
    })?;

    // SAFETY: master is a valid open file descriptor allocated by openpty.
    let mut master_file = unsafe { File::from_raw_fd(master) };

    let mut out_accum = Vec::new();
    let mut buf = [0u8; 1024];
    let mut pw_sent = false;
    let start = std::time::Instant::now();

    while start.elapsed() < std::time::Duration::from_secs(timeout_secs) {
        if let Ok(Some(status)) = child.try_wait() {
            return Ok(status.code().unwrap_or(0));
        }

        let mut pfd = libc::pollfd {
            fd: master,
            events: libc::POLLIN | libc::POLLHUP,
            revents: 0,
        };
        // SAFETY: poll operates on a stack pollfd referencing master.
        let poll_res = unsafe { libc::poll(&mut pfd, 1, 150) };
        if poll_res > 0 {
            if (pfd.revents & libc::POLLIN) != 0 {
                match master_file.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if let Some(chunk) = buf.get(..n) {
                            out_accum.extend_from_slice(chunk);
                        }
                        if !pw_sent {
                            if let Some(pw) = pw {
                                let text = String::from_utf8_lossy(&out_accum);
                                if text.to_lowercase().contains("password") || text.contains(':') {
                                    drop(master_file.write_all(pw.as_bytes()));
                                    drop(master_file.write_all(b"\n"));
                                    drop(master_file.flush());
                                    pw_sent = true;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        // On Linux, PTY read returns EIO when slave closes
                        if e.raw_os_error() == Some(libc::EIO) {
                            break;
                        }
                    }
                }
            } else if (pfd.revents & libc::POLLHUP) != 0 {
                break;
            }
        }
    }

    if let Ok(Some(status)) = child.try_wait() {
        return Ok(status.code().unwrap_or(0));
    }

    // If still running after timeout, kill child process to avoid hanging
    drop(child.kill());
    drop(child.wait());
    Err(format!("operation timed out after {timeout_secs}s"))
}

/// Parses an uncompressed POSIX tar archive containing privileged files into a [`PrivilegedSnapshot`].
pub fn parse_privileged_snapshot(tar_bytes: &[u8]) -> Result<PrivilegedSnapshot, String> {
    let mut reader = pkg::tar::TarReader::new(tar_bytes);
    let mut snapshot = PrivilegedSnapshot::default();

    while let Ok(Some(entry)) = reader.next_entry() {
        if matches!(entry.kind, pkg::tar::Kind::File) {
            let mut data = vec![0u8; entry.size as usize];
            let mut read_bytes = 0;
            while read_bytes < data.len() {
                if let Some(dest) = data.get_mut(read_bytes..) {
                    match reader.read_data(dest) {
                        Ok(0) => break,
                        Ok(n) => read_bytes += n,
                        Err(e) => {
                            return Err(format!(
                                "failed to read tar entry {}: {:?}",
                                entry.path, e
                            ))
                        }
                    }
                } else {
                    break;
                }
            }

            let path = entry.path.trim_start_matches('/').to_string();
            if path == "etc/shadow" {
                snapshot.shadow = Some(String::from_utf8_lossy(&data).into_owned());
            } else if !data.is_empty() {
                snapshot.files.push((path, entry.mode, data));
            }
        }
    }

    Ok(snapshot)
}

/// Directly collects snapshot files without subprocesses when running as root or in test environments.
fn collect_snapshot_directly(shadow: Option<String>) -> Result<PrivilegedSnapshot, String> {
    let mut snapshot = PrivilegedSnapshot {
        shadow,
        files: Vec::new(),
    };

    fn scan_dir(dir: &str, out: &mut Vec<(String, u32, Vec<u8>)>) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Ok(content) = fs::read(&path) {
                        let rel = path
                            .to_string_lossy()
                            .trim_start_matches('/')
                            .to_string();
                        out.push((rel, 0o600, content));
                    }
                } else if path.is_dir() {
                    scan_dir(&path.to_string_lossy(), out);
                }
            }
        }
    }

    scan_dir("/etc/NetworkManager/system-connections", &mut snapshot.files);
    scan_dir("/var/lib/iwd", &mut snapshot.files);
    scan_dir("/var/lib/fprint", &mut snapshot.files);

    if let Ok(zram) = fs::read("/etc/systemd/zram-generator.conf") {
        snapshot
            .files
            .push(("etc/systemd/zram-generator.conf".into(), 0o644, zram));
    }

    Ok(snapshot)
}

/// Collects all privileged system configuration files in a single root operation.
///
/// Gathers `/etc/shadow`, NetworkManager connection profiles, iwd configs, biometric templates
/// from `/var/lib/fprint/`, and zram swap generator configuration.
pub fn collect_privileged_system_files(
    pw: Option<&str>,
    use_pkexec: bool,
) -> Result<PrivilegedSnapshot, String> {
    // 1. Direct read if already readable (e.g. running as root)
    if let Ok(shadow_str) = fs::read_to_string("/etc/shadow") {
        if shadow_str.contains(':') {
            return collect_snapshot_directly(Some(shadow_str));
        }
    }

    // 2. Prepare user-owned temp directory (mode 0700) so we can clean it up regardless of file owner
    let tmp_dir = std::env::temp_dir().join(format!(
        "archstaler-priv-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&tmp_dir)
        .map_err(|e| format!("cannot create temp directory {}: {e}", tmp_dir.display()))?;

    let tmp_tar = tmp_dir.join("snapshot.tar");
    let tmp_tar_str = tmp_tar.to_string_lossy().to_string();

    let tar_targets = [
        "-c",
        "-C",
        "/",
        "--ignore-failed-read",
        "-f",
        &tmp_tar_str,
        "etc/shadow",
        "etc/NetworkManager/system-connections",
        "var/lib/iwd",
        "var/lib/fprint",
        "etc/systemd/zram-generator.conf",
    ];

    let mut success = false;
    let mut last_err = String::new();

    if use_pkexec {
        let mut pk_args = vec!["--disable-internal-agent", "tar"];
        pk_args.extend_from_slice(&tar_targets);
        match Command::new("pkexec").args(&pk_args).output() {
            Ok(out) if out.status.success() => {
                success = true;
            }
            Ok(out) => {
                let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
                if err.contains("No authentication agent found") {
                    last_err = "No desktop Polkit authentication agent is running (e.g. hyprpolkitagent or polkit-gnome). Please enter your root/sudo password above.".into();
                } else if !err.is_empty() {
                    last_err = err;
                } else {
                    last_err = "Polkit authentication was cancelled or failed".into();
                }
            }
            Err(e) => last_err = format!("cannot run pkexec: {e}"),
        }
    } else if let Some(pass) = pw {
        // 1. Try `su -c ...` via PTY first. On systems where PAM sudo has pam_fprintd waiting for a
        // finger sensor, `su` directly verifies the password via pam_unix without delaying or hanging.
        let su_sh_cmd = format!(
            "tar -c -C / --ignore-failed-read -f '{}' etc/shadow etc/NetworkManager/system-connections var/lib/iwd var/lib/fprint etc/systemd/zram-generator.conf",
            tmp_tar_str
        );
        let su_args = ["-c", &su_sh_cmd];
        match run_command_in_pty("su", &su_args, Some(pass), 5) {
            Ok(0) => {
                success = true;
            }
            Ok(code) => {
                last_err = format!("su exited with code {code}");
            }
            Err(e) => {
                last_err = e;
            }
        }

        // 2. If su did not succeed, try sudo with askpass helper under a strict timeout
        if !success {
            let askpass_path = tmp_dir.join("askpass.sh");
            let script = "#!/bin/sh\nprintf \"%s\\n\" \"$ARCHSTALER_AUTH_PW\"\n";
            drop(fs::write(&askpass_path, script));
            let mut perms = fs::metadata(&askpass_path)
                .map(|m| m.permissions())
                .unwrap_or_else(|_| Permissions::from_mode(0o700));
            perms.set_mode(0o700);
            drop(fs::set_permissions(&askpass_path, perms));

            let sudo_cmd = format!(
                "SUDO_ASKPASS='{}' ARCHSTALER_AUTH_PW='{}' sudo -A -k tar -c -C / --ignore-failed-read -f '{}' etc/shadow etc/NetworkManager/system-connections var/lib/iwd var/lib/fprint etc/systemd/zram-generator.conf",
                askpass_path.display(),
                pass.replace('\'', "'\\''"),
                tmp_tar_str
            );
            let sh_args = ["-c", &sudo_cmd];
            match run_command_in_pty("sh", &sh_args, Some(pass), 8) {
                Ok(0) => {
                    success = true;
                }
                Ok(c) => {
                    last_err = format!("sudo authentication failed (exit code {c})");
                }
                Err(e) => {
                    last_err = format!("sudo failed or timed out: {e}");
                }
            }
        }
    }

    if !success {
        drop(fs::remove_dir_all(&tmp_dir));
        return Err(if last_err.is_empty() {
            "authentication failed".into()
        } else {
            last_err
        });
    }

    let tar_bytes = fs::read(&tmp_tar).map_err(|e| {
        drop(fs::remove_dir_all(&tmp_dir));
        format!("cannot read tar snapshot: {e}")
    })?;
    drop(fs::remove_dir_all(&tmp_dir));

    parse_privileged_snapshot(&tar_bytes)
}

/// Reads file bytes, trying direct filesystem access first.
pub fn read_file_privileged(path: &str, _pw: Option<&str>) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))
}

/// Lists file names in a directory, trying direct filesystem access first.
pub fn read_dir_privileged(dir: &str, _pw: Option<&str>) -> Result<Vec<String>, String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("cannot read dir {dir}: {e}"))?;
    let mut names = Vec::new();
    for entry in entries.flatten() {
        if let Ok(name) = entry.file_name().into_string() {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

/// Recursively reads all regular files in a directory tree.
pub fn read_tree_privileged(
    dir: &str,
    _pw: Option<&str>,
) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut files = Vec::new();
    let names = read_dir_privileged(dir, None)?;
    for name in names {
        let sub = format!("{}/{}", dir.trim_end_matches('/'), name);
        let path = Path::new(&sub);
        if let Ok(bytes) = read_file_privileged(&sub, None) {
            files.push((sub, bytes));
        } else if path.is_dir() {
            if let Ok(subfiles) = read_tree_privileged(&sub, None) {
                files.extend(subfiles);
            }
        }
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_regular_file_without_pw() {
        let dir =
            std::env::temp_dir().join(format!("archstaler-priv-test1-{}", std::process::id()));
        drop(std::fs::remove_dir_all(&dir));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("hello.txt");
        fs::write(&file, b"world").unwrap();
        let bytes = read_file_privileged(file.to_str().unwrap(), None).unwrap();
        assert_eq!(bytes, b"world");
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn lists_dir_without_pw() {
        let dir =
            std::env::temp_dir().join(format!("archstaler-priv-test2-{}", std::process::id()));
        drop(std::fs::remove_dir_all(&dir));
        std::fs::create_dir_all(&dir).unwrap();
        let f1 = dir.join("a.txt");
        let f2 = dir.join("b.txt");
        fs::write(&f1, b"").unwrap();
        fs::write(&f2, b"").unwrap();
        let list = read_dir_privileged(dir.to_str().unwrap(), None).unwrap();
        assert_eq!(list, vec!["a.txt".to_string(), "b.txt".to_string()]);
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn parses_empty_tar_snapshot() {
        let empty_tar = [0u8; 1024];
        let snapshot = parse_privileged_snapshot(&empty_tar).unwrap();
        assert_eq!(snapshot.shadow, None);
        assert!(snapshot.files.is_empty());
    }

    #[test]
    fn runs_command_in_pty_successfully() {
        let res = run_command_in_pty("echo", &["test_pty"], None, 2);
        assert_eq!(res, Ok(0));
    }

    #[test]
    fn runs_command_in_pty_times_out() {
        let res = run_command_in_pty("sleep", &["2"], None, 1);
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("timed out"));
    }
}

mod aur;
mod bios;
mod config;
mod coverage;
mod e2e;
mod floppy;
mod iso;
mod keyring;
mod linux_test;
mod presets;
mod qemu;
mod rpi;

use std::path::{Path, PathBuf};
use std::process::Command;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

/// Cargo's build directory: `CARGO_TARGET_DIR` (relative to the root, where every cargo run here starts),
/// else `target/`. The installed `archstaller` command sets it to `~/.cache/archstaller/target`, because the
/// source tree the package installs is read-only.
pub fn target_dir() -> PathBuf {
    match std::env::var_os("CARGO_TARGET_DIR") {
        Some(dir) => root().join(dir),
        None => root().join("target"),
    }
}

/// A path given on the command line. The installed `archstaller` command runs this from the source tree and
/// passes the directory it was started in as `ARCHSTALLER_CWD`; relative paths are taken from there.
fn arg_path(s: &str) -> PathBuf {
    match std::env::var_os("ARCHSTALLER_CWD") {
        Some(cwd) => Path::new(&cwd).join(s),
        None => s.into(),
    }
}

pub fn run(cmd: &mut Command) -> Result<()> {
    // A missing tool (mtools, xorriso, ...) otherwise shows up as a bare "No such file or directory".
    let status = cmd
        .status()
        .map_err(|e| format!("cannot run {}: {e}", cmd.get_program().to_string_lossy()))?;
    if !status.success() {
        return Err(format!("{cmd:?} failed: {status}").into());
    }
    Ok(())
}

pub struct Options {
    pub config: PathBuf,
    pub fault_test: bool,
    /// Kernel overwrites sectors near the end of every disk; QEMU scratch disk only.
    pub selftest: bool,
    /// `--profile`: overrides the config's `build.profile` (default `super-small`).
    pub profile: Option<hostcfg::host::Profile>,
    /// Deprecated `--small`/`--super-small`: acts as `--profile super-small` and warns.
    pub legacy_small: bool,
    /// `xtask size` fails when the ISO exceeds this many bytes.
    pub limit: u64,
    /// Output path of the ISO (default target/archstaller.iso).
    pub out: Option<PathBuf>,
    /// Appended to the config's kernel command line (used by e2e for a serial console).
    pub extra_kernel_params: Vec<String>,
    pub uefi: bool,
    pub headless: bool,
    /// virtio | ahci | nvme | ide (legacy IDE-mode controller, on the i440fx machine)
    pub disk: String,
    /// virtio | e1000 | e1000e | igb | rtl8139 | usb-rndis | none | any QEMU NIC model name (vmxnet3, pcnet, i82559er, ...)
    pub nic: String,
    /// Build with the usb-selftest kernel feature and attach qemu-xhci + usb-storage.
    pub usb: bool,
    /// Build with the usb-tethering kernel feature (iPhone Personal Hotspot over USB; needs a real iPhone).
    pub tethering: bool,
    /// Build with the debug kernel feature: verbose driver and network tracing. Always on for a
    /// dry-run (hardware test) config such as configs/tester.toml.
    pub debug: bool,
    /// `--progress json`: print structured build events (`hostcfg::progress`) on stdout.
    pub progress: bool,
    /// `--workdir DIR`: scratch directory of this build (default `target/`); the GUI gives every build its own.
    pub workdir: Option<PathBuf>,
    /// `--disk-gib N`: size of the e2e test disk in GiB (default 16; the Omarchy preset needs about 24).
    pub disk_gib: u64,
}

fn parse(args: &[String]) -> Result<Options> {
    let mut o = Options {
        config: root().join("configs/config.toml"),
        fault_test: false,
        selftest: false,
        profile: None,
        legacy_small: false,
        limit: 3 << 20,
        out: None,
        extra_kernel_params: Vec::new(),
        uefi: false,
        headless: false,
        disk: "virtio".into(),
        nic: "virtio".into(),
        usb: false,
        tethering: false,
        debug: false,
        progress: false,
        workdir: None,
        disk_gib: 16,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--config" => o.config = arg_path(it.next().ok_or("--config needs a path")?),
            "--fault-test" => o.fault_test = true,
            "--selftest" => o.selftest = true,
            "--out" => o.out = Some(arg_path(it.next().ok_or("--out needs a path")?)),
            "--profile" => {
                o.profile = Some(hostcfg::host::Profile::parse(
                    it.next().ok_or("--profile needs super-small or large")?,
                )?)
            }
            "--small" | "--super-small" => o.legacy_small = true,
            "--limit" => o.limit = it.next().ok_or("--limit needs bytes")?.parse()?,
            "--uefi" => o.uefi = true,
            "--bios" => o.uefi = false,
            "--headless" => o.headless = true,
            "--disk" => o.disk = it.next().ok_or("--disk needs a value")?.clone(),
            "--nic" => o.nic = it.next().ok_or("--nic needs a value")?.clone(),
            "--usb" => o.usb = true,
            "--tethering" => o.tethering = true,
            "--no-tethering" => {} // handled by the presets command
            "--release" => {}
            "--debug" => o.debug = true,
            "--progress" => match it.next().map(String::as_str) {
                Some("json") => o.progress = true,
                _ => return Err("--progress needs the value json".into()),
            },
            "--disk-gib" => o.disk_gib = it.next().ok_or("--disk-gib needs a number")?.parse()?,
            "--workdir" => o.workdir = Some(arg_path(it.next().ok_or("--workdir needs a path")?)),
            other => return Err(format!("unknown option {other}").into()),
        }
    }
    Ok(o)
}

/// Writes `configs/archstaller.schema.json` from the Rust schema; with `--check` only compares.
fn schema(check: bool) -> Result<()> {
    let path = root()
        .join(hostcfg::configs::DIR)
        .join("archstaller.schema.json");
    let text = hostcfg::schema::generate();
    if check {
        if std::fs::read_to_string(&path).map_or(true, |t| t != text) {
            return Err(format!(
                "{} is out of date; run `cargo xtask gen-schema`",
                path.display()
            )
            .into());
        }
        println!("{} is up to date", path.display());
    } else {
        std::fs::write(&path, text)?;
        println!("wrote {}", path.display());
    }
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (cmd, rest) = args.split_first().ok_or("usage: cargo xtask <build|presets|run|size|e2e|test|linux-test|check-presets|gen-schema|coverage|aur-pin|keyring|update-keyring|rpi-build|rpi-run|rpi-test|floppy|floppy-run|floppy-test> [--config FILE] [--out FILE] [--bios|--uefi] [--disk virtio|ahci|nvme|ide] [--nic virtio|e1000|e1000e|igb|rtl8139|usb-rndis|none] [--headless] [--selftest] [--usb] [--tethering|--no-tethering (presets)] [--debug] [--profile super-small|large] [--progress json] [--workdir DIR] [--disk-gib N (e2e)] [--limit BYTES] [--qemu|--all (test)]")?;
    if cmd == "aur-pin" {
        return aur::pin(rest);
    }
    if cmd == "coverage" {
        return coverage::run(rest);
    }
    if cmd == "gen-schema" {
        return schema(rest.iter().any(|a| a == "--check"));
    }
    if cmd == "rpi-build" {
        let opts = parse(rest)?;
        rpi::build_image(&opts)?;
        return Ok(());
    }
    if cmd == "rpi-run" {
        let board = rest.first().map(|s| s.as_str()).unwrap_or("raspi3b");
        return rpi::run(board);
    }
    if cmd == "rpi-test" {
        let board = rest.first().map(|s| s.as_str()).unwrap_or("raspi3b");
        return rpi::test(board);
    }
    if cmd == "test" {
        let mut c = Command::new("cargo");
        c.arg("test").arg("--workspace");
        run(&mut c)?;
        if rest.iter().any(|a| a == "--all" || a == "--qemu") {
            let filtered_rest: Vec<String> = rest
                .iter()
                .filter(|a| a.as_str() != "--all" && a.as_str() != "--qemu")
                .cloned()
                .collect();
            let opts = parse(&filtered_rest)?;
            linux_test::run_test()?;
            floppy::test(&opts)?;
            rpi::test("raspi3b")?;
        }
        return Ok(());
    }
    let opts = parse(rest)?;
    match cmd.as_str() {
        "build" => {
            iso::build(&opts)?;
        }
        "run" => {
            let iso = iso::build(&opts)?;
            qemu::run_iso(&iso, &opts)?;
        }
        "floppy" => {
            floppy::build(&opts)?;
        }
        "floppy-run" => {
            floppy::run(&opts)?;
        }
        "floppy-test" => {
            floppy::test(&opts)?;
        }
        // Preset ISOs are for real hardware, so they include USB tethering unless --no-tethering is given.
        "presets" => {
            let mut o = opts;
            o.tethering = !rest.iter().any(|a| a == "--no-tethering");
            presets::build_all(&o)?
        }
        "check-presets" => presets::check()?,
        "e2e" => e2e::run_e2e(&opts)?,
        "linux-test" => linux_test::run_test()?,
        "update-keyring" => keyring::update()?,
        "keyring" => {
            keyring::build()?;
        }
        "size" => iso::size(&opts)?,
        other => return Err(format!("unknown command {other}").into()),
    }
    Ok(())
}

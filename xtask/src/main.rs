mod bios;
mod e2e;
mod iso;
mod keyring;
mod presets;
mod linux_test;
mod lua;
mod qemu;

use std::path::{Path, PathBuf};
use std::process::Command;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

pub fn run(cmd: &mut Command) -> Result<()> {
    let status = cmd.status()?;
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
    /// Output path of the ISO (default target/archstaler.iso).
    pub out: Option<PathBuf>,
    /// Appended to the config's kernel command line (used by e2e for a serial console).
    pub extra_kernel_params: Vec<String>,
    pub uefi: bool,
    pub headless: bool,
    /// virtio | ahci | nvme | ide (legacy IDE-mode controller, on the i440fx machine)
    pub disk: String,
    /// virtio | e1000 | e1000e | igb | rtl8139 | usb-rndis | none
    pub nic: String,
    /// Build with the usb-selftest kernel feature and attach qemu-xhci + usb-storage.
    pub usb: bool,
    /// Build with the usb-tethering kernel feature (iPhone Personal Hotspot over USB; needs a real iPhone).
    pub tethering: bool,
    /// Build with the debug kernel feature: verbose driver and network tracing. Always on for a
    /// dry-run (hardware test) config such as presets/tester.lua.
    pub debug: bool,
    /// `--progress json`: print structured build events (`hostcfg::progress`) on stdout.
    pub progress: bool,
    /// `--workdir DIR`: scratch directory of this build (default `target/`); the GUI gives every build its own.
    pub workdir: Option<PathBuf>,
}

fn parse(args: &[String]) -> Result<Options> {
    let mut o = Options {
        config: root().join("examples/config.lua"),
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
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--config" => o.config = it.next().ok_or("--config needs a path")?.into(),
            "--fault-test" => o.fault_test = true,
            "--selftest" => o.selftest = true,
            "--out" => o.out = Some(it.next().ok_or("--out needs a path")?.into()),
            "--profile" => o.profile = Some(hostcfg::host::Profile::parse(it.next().ok_or("--profile needs super-small or large")?)?),
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
            "--debug" => o.debug = true,
            "--progress" => match it.next().map(String::as_str) {
                Some("json") => o.progress = true,
                _ => return Err("--progress needs the value json".into()),
            },
            "--workdir" => o.workdir = Some(it.next().ok_or("--workdir needs a path")?.into()),
            other => return Err(format!("unknown option {other}").into()),
        }
    }
    Ok(o)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (cmd, rest) = args.split_first().ok_or("usage: cargo xtask <build|presets|run|size|e2e|linux-test|check-presets|keyring|update-keyring> [--config FILE] [--out FILE] [--bios|--uefi] [--disk virtio|ahci|nvme|ide] [--nic virtio|e1000|e1000e|igb|rtl8139|usb-rndis|none] [--headless] [--selftest] [--usb] [--tethering|--no-tethering (presets)] [--debug] [--profile super-small|large] [--progress json] [--workdir DIR] [--limit BYTES]")?;
    let opts = parse(rest)?;
    match cmd.as_str() {
        "build" => {
            iso::build(&opts)?;
        }
        "run" => {
            let iso = iso::build(&opts)?;
            qemu::run_iso(&iso, &opts)?;
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

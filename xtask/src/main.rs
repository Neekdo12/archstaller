mod iso;
mod limine;
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
    pub uefi: bool,
    pub headless: bool,
}

fn parse(args: &[String]) -> Result<Options> {
    let mut o = Options {
        config: root().join("examples/config.lua"),
        fault_test: false,
        uefi: false,
        headless: false,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--config" => o.config = it.next().ok_or("--config needs a path")?.into(),
            "--fault-test" => o.fault_test = true,
            "--uefi" => o.uefi = true,
            "--bios" => o.uefi = false,
            "--headless" => o.headless = true,
            other => return Err(format!("unknown option {other}").into()),
        }
    }
    Ok(o)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (cmd, rest) = args.split_first().ok_or("usage: cargo xtask <build|run|size> [options]")?;
    let opts = parse(rest)?;
    match cmd.as_str() {
        "build" => {
            iso::build(&opts)?;
        }
        "run" => {
            let iso = iso::build(&opts)?;
            qemu::run_iso(&iso, &opts)?;
        }
        "size" => iso::size(&opts)?,
        other => return Err(format!("unknown command {other}").into()),
    }
    Ok(())
}

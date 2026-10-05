//! Preset configs in presets/*.lua: dependency check and one ISO per preset.
use crate::{iso, lua, root, Options, Result};
use std::path::PathBuf;

fn preset_files() -> Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(root().join("presets"))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "lua") && p.file_stem().is_some_and(|s| s != "common"))
        .collect();
    files.sort();
    Ok(files)
}

fn stem(p: &std::path::Path) -> String {
    p.file_stem().unwrap().to_string_lossy().into_owned()
}

/// Resolves every preset against the host's sync databases and reports what would be installed.
pub fn check() -> Result<()> {
    let dbs = hostcfg::resolve::local_dbs()?;
    let mut failed = false;
    for f in preset_files()? {
        let cfg = lua::load_config(&f)?;
        match hostcfg::resolve::resolve(&cfg, &dbs) {
            Ok(r) => {
                println!("{:<10} {:>4} packages, {:>5} MiB to download, {} providers configured", stem(&f), r.packages.len(), r.download_bytes() >> 20, cfg.providers.len());
                for a in &r.ambiguities {
                    println!("             ambiguous {} -> {} (of {})", a.dep, a.chosen, a.candidates.join(", "));
                }
            }
            Err(e) => {
                println!("{:<10} FAILED: {e}", stem(&f));
                failed = true;
            }
        }
    }
    if failed {
        return Err("some presets do not resolve".into());
    }
    Ok(())
}

/// Builds target/isos/archstaler-<preset>.iso for every preset.
pub fn build_all(opts: &Options) -> Result<()> {
    let dir = root().join("target/isos");
    for f in preset_files()? {
        let name = stem(&f);
        let o = Options {
            config: f.clone(),
            out: Some(dir.join(format!("archstaler-{name}.iso"))),
            fault_test: false,
            selftest: false,
            profile: opts.profile,
            legacy_small: opts.legacy_small,
            limit: opts.limit,
            extra_kernel_params: Vec::new(),
            uefi: false,
            headless: false,
            disk: "virtio".into(),
            nic: "virtio".into(),
            usb: false,
            tethering: opts.tethering,
            debug: opts.debug,
            progress: false,
            workdir: None,
        };
        let iso = iso::build(&o)?;
        println!("{name}: {} ({} bytes)", iso.display(), std::fs::metadata(&iso)?.len());
    }
    Ok(())
}

//! QEMU Virtual Machine Configuration and Runner
//!
//! Provides orchestration for graphical and headless QEMU E2E test runs:
//! - Firmware selection (UEFI / OVMF vs. SeaBIOS)
//! - Disk controller selection (VirtIO-Block, NVMe, AHCI, IDE)
//! - Network interface selection (VirtIO, e1000, rtl8139, e100, pcnet)
//! - Display backends (GTK, SDL, VNC, None)
//! - Sparse disk provisioning and process supervision

use crate::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

const OVMF_DIR: &str = "/usr/share/edk2/x64";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Firmware {
    #[default]
    Uefi,
    Bios,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DiskController {
    #[default]
    Virtio,
    Nvme,
    Ahci,
    Ide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum NicModel {
    #[default]
    Virtio,
    E1000,
    E1000e,
    Rtl8139,
    E100,
    Pcnet,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DisplayBackend {
    #[default]
    Gtk,
    Sdl,
    Vnc,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QemuConfig {
    pub iso_path: PathBuf,
    pub disk_path: PathBuf,
    pub disk_size_gib: u32,
    pub keep_disk: bool,
    pub firmware: Firmware,
    pub disk_controller: DiskController,
    pub nic: NicModel,
    pub ram_mib: u32,
    pub cpu_cores: u32,
    pub display: DisplayBackend,
    pub enable_kvm: bool,
    pub extra_args: Vec<String>,
}

impl Default for QemuConfig {
    fn default() -> Self {
        Self {
            iso_path: PathBuf::from("target/archstaller.iso"),
            disk_path: PathBuf::from("target/test-disk.img"),
            disk_size_gib: 20,
            keep_disk: false,
            firmware: Firmware::Uefi,
            disk_controller: DiskController::Virtio,
            nic: NicModel::Virtio,
            ram_mib: 4096,
            cpu_cores: 2,
            display: DisplayBackend::Gtk,
            enable_kvm: true,
            extra_args: Vec::new(),
        }
    }
}

impl QemuConfig {
    pub fn validate(&self) -> Result<()> {
        if self.ram_mib < 256 {
            return Err("RAM must be at least 256 MiB".into());
        }
        if self.cpu_cores < 1 {
            return Err("CPU cores must be at least 1".into());
        }
        if self.disk_size_gib < 1 {
            return Err("disk size must be at least 1 GiB".into());
        }
        if self.iso_path.as_os_str().is_empty() {
            return Err("ISO path must not be empty".into());
        }
        Ok(())
    }

    pub fn create_disk_image(&self) -> Result<()> {
        if let Some(parent) = self.disk_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let f = std::fs::File::create(&self.disk_path)?;
        let bytes = (self.disk_size_gib as u64) << 30;
        f.set_len(bytes)?;
        Ok(())
    }

    pub fn build_command(&self) -> Result<Command> {
        self.validate()?;

        let mut cmd = Command::new("qemu-system-x86_64");

        let machine = if self.disk_controller == DiskController::Ide {
            "pc"
        } else {
            "q35"
        };
        cmd.args(["-machine", machine]);
        cmd.args(["-m", &format!("{}M", self.ram_mib)]);
        cmd.args(["-smp", &format!("{}", self.cpu_cores)]);
        cmd.args(["-serial", "stdio", "-no-reboot"]);

        if self.enable_kvm && Path::new("/dev/kvm").exists() {
            cmd.args(["-enable-kvm", "-cpu", "host"]);
        }

        match self.display {
            DisplayBackend::Gtk => {
                cmd.args(["-display", "gtk,gl=on"]);
            }
            DisplayBackend::Sdl => {
                cmd.args(["-display", "sdl"]);
            }
            DisplayBackend::Vnc => {
                cmd.args(["-display", "vnc=:1"]);
            }
            DisplayBackend::None => {
                cmd.args(["-display", "none"]);
            }
        }

        if self.firmware == Firmware::Uefi {
            let ovmf_code = format!("{OVMF_DIR}/OVMF_CODE.4m.fd");
            let ovmf_vars = format!("{OVMF_DIR}/OVMF_VARS.4m.fd");

            let target_vars = self
                .disk_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join("ovmf_vars.fd");

            if Path::new(&ovmf_vars).exists() {
                drop(std::fs::copy(&ovmf_vars, &target_vars));
            }

            cmd.arg("-drive").arg(format!(
                "if=pflash,format=raw,readonly=on,file={ovmf_code}"
            ));
            cmd.arg("-drive")
                .arg(format!("if=pflash,format=raw,file={}", target_vars.display()));
        }

        cmd.args(["-device", "isa-debug-exit,iobase=0xf4,iosize=0x04"]);
        cmd.arg("-cdrom").arg(&self.iso_path);

        cmd.arg("-drive").arg(format!(
            "if=none,id=d0,format=raw,file={}",
            self.disk_path.display()
        ));

        match self.disk_controller {
            DiskController::Virtio => {
                cmd.args(["-device", "virtio-blk-pci,drive=d0,serial=TESTDISK0"]);
            }
            DiskController::Ahci => {
                cmd.args([
                    "-device",
                    "ich9-ahci,id=ahci",
                    "-device",
                    "ide-hd,drive=d0,bus=ahci.0,serial=TESTDISK0",
                ]);
            }
            DiskController::Nvme => {
                cmd.args(["-device", "nvme,drive=d0,serial=TESTDISK0"]);
            }
            DiskController::Ide => {
                cmd.args(["-device", "ide-hd,drive=d0,bus=ide.0,serial=TESTDISK0"]);
            }
        }

        if self.nic != NicModel::None {
            cmd.args(["-netdev", "user,id=n0"]);
            match self.nic {
                NicModel::Virtio => {
                    cmd.args(["-device", "virtio-net-pci,netdev=n0,disable-legacy=on"]);
                }
                NicModel::E1000 => {
                    cmd.args(["-device", "e1000,netdev=n0"]);
                }
                NicModel::E1000e => {
                    cmd.args(["-device", "e1000e,netdev=n0"]);
                }
                NicModel::Rtl8139 => {
                    cmd.args(["-device", "rtl8139,netdev=n0"]);
                }
                NicModel::E100 => {
                    cmd.args(["-device", "i82559er,netdev=n0"]);
                }
                NicModel::Pcnet => {
                    cmd.args(["-device", "pcnet,netdev=n0"]);
                }
                NicModel::None => {}
            }
        }

        for arg in &self.extra_args {
            cmd.arg(arg);
        }

        Ok(cmd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_rules() {
        let mut cfg = QemuConfig {
            ram_mib: 128, // < 256 MiB
            ..Default::default()
        };
        assert!(cfg.validate().is_err());

        cfg.ram_mib = 2048;
        cfg.cpu_cores = 0;
        assert!(cfg.validate().is_err());

        cfg.cpu_cores = 2;
        cfg.disk_size_gib = 0;
        assert!(cfg.validate().is_err());

        cfg.disk_size_gib = 20;
        cfg.validate().unwrap();
    }

    #[test]
    fn builds_uefi_virtio_gtk_command() {
        let cfg = QemuConfig {
            iso_path: PathBuf::from("/tmp/test.iso"),
            disk_path: PathBuf::from("/tmp/test.img"),
            disk_size_gib: 20,
            keep_disk: false,
            firmware: Firmware::Uefi,
            disk_controller: DiskController::Virtio,
            nic: NicModel::Virtio,
            ram_mib: 4096,
            cpu_cores: 4,
            display: DisplayBackend::Gtk,
            enable_kvm: false,
            extra_args: vec!["-snapshot".to_string()],
        };

        let cmd = cfg.build_command().unwrap();
        let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().to_string()).collect();

        assert!(args.contains(&"-m".to_string()));
        assert!(args.contains(&"4096M".to_string()));
        assert!(args.contains(&"-smp".to_string()));
        assert!(args.contains(&"4".to_string()));
        assert!(args.contains(&"virtio-blk-pci,drive=d0,serial=TESTDISK0".to_string()));
        assert!(args.contains(&"virtio-net-pci,netdev=n0,disable-legacy=on".to_string()));
        assert!(args.contains(&"gtk,gl=on".to_string()));
        assert!(args.contains(&"-snapshot".to_string()));
    }

    #[test]
    fn builds_bios_ide_sdl_command() {
        let cfg = QemuConfig {
            iso_path: PathBuf::from("/tmp/test.iso"),
            disk_path: PathBuf::from("/tmp/test.img"),
            disk_size_gib: 10,
            keep_disk: true,
            firmware: Firmware::Bios,
            disk_controller: DiskController::Ide,
            nic: NicModel::E1000,
            ram_mib: 2048,
            cpu_cores: 1,
            display: DisplayBackend::Sdl,
            enable_kvm: false,
            extra_args: Vec::new(),
        };

        let cmd = cfg.build_command().unwrap();
        let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().to_string()).collect();

        assert!(args.contains(&"pc".to_string())); // machine pc for IDE
        assert!(args.contains(&"ide-hd,drive=d0,bus=ide.0,serial=TESTDISK0".to_string()));
        assert!(args.contains(&"e1000,netdev=n0".to_string()));
        assert!(args.contains(&"sdl".to_string()));
        // BIOS should not have pflash
        assert!(!args.iter().any(|a| a.contains("if=pflash")));
    }

    #[test]
    fn builds_all_nic_models() {
        for (nic, dev_expected) in [
            (NicModel::E1000e, "e1000e,netdev=n0"),
            (NicModel::Rtl8139, "rtl8139,netdev=n0"),
            (NicModel::E100, "i82559er,netdev=n0"),
            (NicModel::Pcnet, "pcnet,netdev=n0"),
        ] {
            let cfg = QemuConfig {
                nic,
                ..Default::default()
            };
            let cmd = cfg.build_command().unwrap();
            let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().to_string()).collect();
            assert!(args.contains(&dev_expected.to_string()), "testing {nic:?}");
        }

        let none_cfg = QemuConfig {
            nic: NicModel::None,
            ..Default::default()
        };
        let none_cmd = none_cfg.build_command().unwrap();
        let none_args: Vec<String> = none_cmd.get_args().map(|a| a.to_string_lossy().to_string()).collect();
        assert!(!none_args.contains(&"-netdev".to_string()));
    }

    #[test]
    fn disk_image_provisioning() {
        let temp_dir = std::env::temp_dir().join(format!("qemu-test-{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let disk_path = temp_dir.join("sparse-disk.img");

        let cfg = QemuConfig {
            disk_path: disk_path.clone(),
            disk_size_gib: 5,
            ..Default::default()
        };

        cfg.create_disk_image().unwrap();
        assert!(disk_path.exists());
        let meta = std::fs::metadata(&disk_path).unwrap();
        assert_eq!(meta.len(), 5 * 1024 * 1024 * 1024);

        drop(std::fs::remove_dir_all(&temp_dir));
    }
}

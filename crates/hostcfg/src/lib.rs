//! Host-side configuration model, shared by `xtask` and the desktop GUI so that both reject exactly
//! the same configs and build exactly the same ISOs. Std only: nothing here reaches the kernel.
//!
//! * [`toml`]: evaluating a TOML config into the installer's [`config::Config`] plus the host-only
//!   [`host::HostConfig`] (`build`, `installer_drivers`).
//! * [`host`]: build profiles, the installer driver catalogue and their validation.

pub mod archives;
pub mod asconfig;
pub mod autoload;
pub mod bluetooth;
pub mod configs;
pub mod fprint;
pub mod host;
pub mod network;
pub mod password;
pub mod privilege;
pub mod progress;
pub mod resolve;
pub mod schema;
pub mod scripts;
pub mod services;
pub mod ssh;
pub mod system;
pub mod system_files;
pub mod toml;
pub mod users;
pub mod writer;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

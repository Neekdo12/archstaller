//! Host-side configuration model, shared by `xtask` and the desktop GUI so that both reject exactly
//! the same configs and build exactly the same ISOs. Std only: nothing here reaches the kernel.
//!
//! * [`lua`]: evaluating a Lua config into the installer's [`config::Config`] plus the host-only
//!   [`host::HostConfig`] (`build`, `installer_drivers`).
//! * [`host`]: build profiles, the installer driver catalogue and their validation.

pub mod host;
pub mod lua;
pub mod password;
pub mod progress;
pub mod resolve;
pub mod scripts;
pub mod writer;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

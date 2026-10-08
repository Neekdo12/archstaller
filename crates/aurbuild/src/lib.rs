//! Host side of AUR support (plans-implement/aur.md): talk to the AUR, review a recipe, pin it, plan the install.
//! Nothing here builds a package: the installed system does that at its first boot
//! (`firstboot/archstaler-aur.sh`), from the commit and the tree digest this crate produced.
pub mod digest;
pub mod plan;
#[cfg(feature = "net")]
pub mod rpc;
pub mod srcinfo;

pub type Result<T> = std::result::Result<T, String>;

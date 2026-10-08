//! One module per page of the window. Each `build` fills the page's box from the current document;
//! controls write back through `App::edit`.
pub mod build_page;
pub mod disk;
pub mod iso;
pub mod lua;
pub mod packages;
pub mod scripts;
pub mod services;
pub mod system;
pub mod users;

//! Windows integration shared by the installer and native launcher.
#[cfg(windows)]
mod handoff;
#[cfg(windows)]
pub use handoff::*;
#[cfg(windows)]
mod shell;
#[cfg(windows)]
pub use shell::{install_root, refresh_shortcuts};
#[cfg(windows)]
mod tasks;
#[cfg(windows)]
pub use tasks::{autostart_enabled, set_autostart};
#[cfg(windows)]
mod maintenance;
#[cfg(windows)]
pub use maintenance::{install_application, uninstall_application};

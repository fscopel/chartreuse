//! Pure logic of the Linux backends: no system calls and no Linux-only crates,
//! only `chartreuse-core` types, so test builds on every host compile it and
//! its unit tests run on macOS and Windows too.

pub mod autostart;
pub mod ewmh;
pub mod file_chooser;
pub mod keysym;
pub mod randr;
pub mod screenshot;
pub mod tray;
pub mod wl_output;
pub mod xgrab;
pub mod ximage;
pub mod xsettings;

//! Capture modes.

use std::fmt;

/// What a capture targets. Each mode has its own global hotkey and menu entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CaptureMode {
    /// Every display, straight into the editor.
    Display,
    /// A rectangle dragged out in rectangle-selection mode, or a window picked
    /// in window-selection mode. Starts in rectangle selection; Space switches
    /// between the two.
    Selection,
}

impl CaptureMode {
    pub const ALL: [Self; 2] = [Self::Display, Self::Selection];
}

/// The user-facing action name, for example "Capture display".
impl fmt::Display for CaptureMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Display => "Capture display",
            Self::Selection => "Capture selection",
        })
    }
}

//! Fake [`OverlayWindowStyle`]: counts styled windows.

use chartreuse_core::display::DisplayInfo;
use chartreuse_core::Result;

use super::Fake;
use crate::overlay_style::{NativeWindow, OverlayWindowStyle};

impl OverlayWindowStyle for Fake {
    fn apply(&self, _window: NativeWindow<'_>, _display: &DisplayInfo) -> Result<()> {
        self.state.lock().overlays_styled += 1;
        Ok(())
    }
}

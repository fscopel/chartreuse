//! Screen and window capture.

use chartreuse_core::display::DisplayInfo;
use chartreuse_core::image::Image;
use chartreuse_core::window::WindowId;
use chartreuse_core::Result;
use futures::future::BoxFuture;

/// A native-resolution capture of one display.
#[derive(Debug, Clone)]
pub struct DisplayCapture {
    /// The display as it was when captured.
    pub display: DisplayInfo,
    /// The display's contents; `image.size() == display.pixel_size`.
    pub image: Image,
}

/// Captures displays and windows at native resolution.
///
/// Call the methods on the main thread (from iced `update`). They start the capture
/// and return a `Send + 'static` future, meant for `Task::perform`, that resolves
/// when the capture finishes.
pub trait Capture {
    /// Captures every connected display.
    ///
    /// Fails with [`Error::PermissionDenied`](chartreuse_core::Error::PermissionDenied)
    /// when the OS withholds screen contents.
    fn capture_displays(&self) -> BoxFuture<'static, Result<Vec<DisplayCapture>>>;

    /// Captures one window, as listed by [`WindowList`](crate::WindowList).
    fn capture_window(&self, window: WindowId) -> BoxFuture<'static, Result<Image>>;

    /// Captures what the user picks in the platform's own capture interface,
    /// on platforms where Chartreuse cannot select by itself; `None` (the
    /// default) where there is no such interface. Resolves to `None` if the
    /// user cancels. The interface opens only once the future is polled, so a
    /// caller may drop it unpolled after learning there is one.
    ///
    /// Wayland has one, the Screenshot portal's interactive mode. Chartreuse
    /// cannot list windows there, so window selection hands over to it, and its
    /// overlays cover a single output only, so selections on several do too.
    fn capture_interactively(&self) -> Option<BoxFuture<'static, Result<Option<Image>>>> {
        None
    }
}

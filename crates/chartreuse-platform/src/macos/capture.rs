//! macOS: display and window capture through ScreenCaptureKit.
//!
//! Display capture is track 2A's, window capture track 2G's.
//!
//! # Display capture flow
//!
//! 1. On the main thread, [`MacosDisplays`] describes the connected displays, and
//!    `CGPreflightScreenCaptureAccess` rules out a missing grant up front.
//! 2. [`with_shareable_content`] asks ScreenCaptureKit for the shareable content.
//!    Its completion handler runs on a ScreenCaptureKit queue; the closure it runs
//!    there matches every display to its `SCDisplay` by `CGDirectDisplayID` and
//!    starts one [`capture_image`] per display, excluding Chartreuse's own windows.
//! 3. Each capture's completion handler converts the `CGImage` to an [`Image`]
//!    ([`image_from_cg`], from the shared `cgimage` module) and resolves a
//!    oneshot channel, so the returned future only ever holds `Send` data.
//!
//! # Window capture flow
//!
//! The window is captured directly, so it comes out whole even where other
//! windows cover it, with its shadow and with transparent rounded corners.
//!
//! 1. On the main thread, as above: the displays and the grant.
//! 2. On ScreenCaptureKit's queue, [`start_window_screenshot`] finds the
//!    `SCWindow` with the requested `CGWindowID` (the id the window list hands
//!    out) and captures it through a desktop-independent-window filter. The
//!    output is sized for the window's native resolution — its frame times the
//!    backing scale of the display showing most of it ([`backing_scale`]) — plus
//!    room for the shadow ([`SHADOW_MARGIN`], [`window_canvas_size`]).
//! 3. The `CGImage` is converted at its own size ([`cg_image_to_image`]), and
//!    [`trim_transparent_edges`] crops the unused canvas away.
//!
//! The private helpers ([`with_shareable_content`], [`stream_configuration`],
//! [`capture_image`], [`own_application`], [`error_from_ns`]) are shared with
//! window capture. The window list (`window_list.rs`) reuses
//! [`with_shareable_content`], [`own_pid`], [`permission_denied`] and
//! [`check_not_withheld`].
//!
//! # Withheld contents
//!
//! macOS can withhold screen contents without reporting an error: a grant that
//! was revoked while the app runs, or that stopped applying to a rebuilt
//! ad-hoc-signed binary, can yield captures with only the wallpaper (other apps'
//! windows missing) or fully transparent images. Two heuristics map those cases to
//! [`Error::PermissionDenied`] so the app shows the Screen Recording guidance:
//!
//! - [`ForeignWindows::withheld`]: ScreenCaptureKit lists no on-screen windows
//!   from other processes while the window server (`CGWindowListCopyWindowInfo`,
//!   which works without the permission) does. With a working grant
//!   ScreenCaptureKit always lists some, such as the menu bar and the Dock's
//!   wallpaper windows.
//! - [`all_blank`]: every display captured entirely fully transparent (alpha 0).
//!   The screen itself is always opaque, so real content never looks like that.
//!   Uniform opaque frames are deliberately accepted: a full-screen app (a video
//!   paused on black, a black slide) or an auto-hidden menu bar over a
//!   solid-color wallpaper legitimately captures as a single color. Likewise, a
//!   window capture with no visible pixel at all ([`trim_transparent_edges`]
//!   finds nothing) is taken as withheld.

use std::process;

use block2::RcBlock;
use chartreuse_core::display::{DisplayId, DisplayInfo};
use chartreuse_core::geometry::{LogicalRect, LogicalSize, PhysicalSize, ScaleFactor};
use chartreuse_core::image::Image;
use chartreuse_core::permission::Permission;
use chartreuse_core::window::WindowId;
use chartreuse_core::{Error, Result};
use futures::channel::oneshot;
use futures::future::{self, BoxFuture, FutureExt};
use objc2::rc::Retained;
use objc2::AllocAnyThread;
use objc2_core_foundation::{CFArray, CFDictionary, CFNumber, CFRetained, CFString, CFType};
use objc2_core_graphics::{
    kCGColorSpaceSRGB, kCGNullWindowID, kCGWindowOwnerPID, CGImage, CGPreflightScreenCaptureAccess,
    CGWindowListCopyWindowInfo, CGWindowListOption,
};
use objc2_foundation::{NSArray, NSError, NSInteger};
use objc2_screen_capture_kit::{
    SCCaptureResolutionType, SCContentFilter, SCRunningApplication, SCScreenshotManager,
    SCShareableContent, SCStreamConfiguration, SCStreamErrorCode, SCStreamErrorDomain,
};
use parking_lot::Mutex;

use super::cgimage::{cg_image_to_image, image_from_cg};
use super::displays::MacosDisplays;
use crate::capture::{Capture, DisplayCapture};
use crate::displays::Displays;

/// The macOS [`Capture`] backend.
#[derive(Debug, Default)]
pub struct MacosCapture;

impl MacosCapture {
    pub fn new() -> Self {
        Self
    }
}

impl Capture for MacosCapture {
    fn capture_displays(&self) -> BoxFuture<'static, Result<Vec<DisplayCapture>>> {
        start_display_capture().unwrap_or_else(|error| future::ready(Err(error)).boxed())
    }

    fn capture_window(&self, window: WindowId) -> BoxFuture<'static, Result<Image>> {
        start_window_capture(window).unwrap_or_else(|error| future::ready(Err(error)).boxed())
    }
}

/// The error for a missing, revoked, or silently withheld Screen Recording grant.
pub(super) fn permission_denied() -> Error {
    Error::PermissionDenied(Permission::ScreenRecording)
}

// ---------------------------------------------------------------------------
// Display capture
// ---------------------------------------------------------------------------

/// Does the main-thread part of [`Capture::capture_displays`] and returns the
/// future that finishes it.
fn start_display_capture() -> Result<BoxFuture<'static, Result<Vec<DisplayCapture>>>> {
    // NSScreen requires the main thread, so the display model is read here, before
    // anything moves to ScreenCaptureKit's queues.
    let displays = MacosDisplays::new().displays()?;
    if !CGPreflightScreenCaptureAccess() {
        return Err(permission_denied());
    }
    let started = with_shareable_content(move |content| start_captures(content, displays));
    Ok(async move {
        let pending = started.await?;
        let mut captures = Vec::with_capacity(pending.len());
        for (display, image) in pending {
            captures.push(DisplayCapture {
                display,
                image: image.await?,
            });
        }
        if all_blank(captures.iter().map(|capture| &capture.image)) {
            tracing::warn!(
                "every display captured fully transparent; treating Screen Recording as withheld"
            );
            return Err(permission_denied());
        }
        Ok(captures)
    }
    .boxed())
}

/// A display whose capture has started.
type PendingCapture = (DisplayInfo, BoxFuture<'static, Result<Image>>);

/// Runs on ScreenCaptureKit's queue: checks for withheld contents, then starts one
/// capture per display.
fn start_captures(
    content: &SCShareableContent,
    displays: Vec<DisplayInfo>,
) -> Result<Vec<PendingCapture>> {
    let own = own_application(content);
    check_not_withheld(content)?;

    // SAFETY: `displays` is a plain property getter on a valid object.
    let sc_displays = unsafe { content.displays() };
    let ids: Vec<u32> = sc_displays
        .iter()
        // SAFETY: plain property getter on a valid `SCDisplay`.
        .map(|display| unsafe { display.displayID() })
        .collect();
    let excluded: Retained<NSArray<SCRunningApplication>> = match &own {
        Some(own) => NSArray::from_retained_slice(std::slice::from_ref(own)),
        None => NSArray::new(),
    };
    let no_windows = NSArray::new();

    let matched = match_displays(displays, &ids)?;
    Ok(matched
        .into_iter()
        .map(|(display, index)| {
            let sc_display = sc_displays.objectAtIndex(index);
            // SAFETY: a fresh allocation initialized with valid display and arrays.
            let filter = unsafe {
                SCContentFilter::initWithDisplay_excludingApplications_exceptingWindows(
                    SCContentFilter::alloc(),
                    &sc_display,
                    &excluded,
                    &no_windows,
                )
            };
            let image = capture_image(
                &filter,
                &stream_configuration(display.pixel_size),
                Some(display.pixel_size),
            );
            (display, image)
        })
        .collect())
}

/// Pairs every display of the display model with the index of its `SCDisplay` in
/// `shareable_ids` (the `CGDirectDisplayID`s ScreenCaptureKit listed), keeping the
/// display model's order. Shareable displays the model lacks are ignored.
///
/// Fails when a display of the model is missing from ScreenCaptureKit's list,
/// which happens when it is asleep (ScreenCaptureKit lists only active displays,
/// while `NSScreen` also lists sleeping ones) or when displays change between the
/// two enumerations.
fn match_displays(
    displays: Vec<DisplayInfo>,
    shareable_ids: &[u32],
) -> Result<Vec<(DisplayInfo, usize)>> {
    displays
        .into_iter()
        .map(|display| {
            match shareable_ids
                .iter()
                .position(|&id| DisplayId(u64::from(id)) == display.id)
            {
                Some(index) => Ok((display, index)),
                None => Err(Error::Platform(format!(
                    "display {} ({}) is not available to ScreenCaptureKit; \
                     it may be asleep, or the display configuration changed",
                    display.id.0, display.name
                ))),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Window capture
// ---------------------------------------------------------------------------

/// Room for a window's shadow, in points on every side of its frame.
///
/// ScreenCaptureKit reports a window's frame without its shadow (both
/// `SCWindow.frame` and `SCContentFilter.contentRect`), yet with shadows included
/// it fits the shadowed window into the output size, shrinking the window when
/// the output is only frame-sized. The output is therefore made larger than any
/// shadow (an active window's reaches about 75 points below it and 56 to the
/// sides), which ScreenCaptureKit fills without scaling, and
/// [`trim_transparent_edges`] cuts it back to the shadow's extent.
const SHADOW_MARGIN: f64 = 128.0;

/// Does the main-thread part of [`Capture::capture_window`] and returns the
/// future that finishes it.
fn start_window_capture(window: WindowId) -> Result<BoxFuture<'static, Result<Image>>> {
    // NSScreen requires the main thread; the displays decide the capture scale.
    let displays = MacosDisplays::new().displays()?;
    if !CGPreflightScreenCaptureAccess() {
        return Err(permission_denied());
    }
    let started =
        with_shareable_content(move |content| start_window_screenshot(content, window, &displays));
    Ok(async move {
        let image = started.await?.await?;
        trim_transparent_edges(&image).ok_or_else(|| {
            tracing::warn!(
                window = window.0,
                "the window captured fully transparent; treating Screen Recording as withheld"
            );
            permission_denied()
        })
    }
    .boxed())
}

/// Runs on ScreenCaptureKit's queue: finds `window` and starts its screenshot at
/// the native resolution of the display it is mostly on, shadow included.
fn start_window_screenshot(
    content: &SCShareableContent,
    window: WindowId,
    displays: &[DisplayInfo],
) -> Result<BoxFuture<'static, Result<Image>>> {
    // SAFETY: `windows` and `windowID` are plain property getters on valid objects.
    let sc_window = unsafe { content.windows() }
        .iter()
        .find(|candidate| u64::from(unsafe { candidate.windowID() }) == window.0)
        .ok_or_else(|| missing_window(content, window))?;
    // SAFETY: plain property getter on a valid `SCWindow`. Its frame is in the
    // global logical space, like the window list's bounds.
    let frame = unsafe { sc_window.frame() };
    let frame = LogicalRect::new(
        frame.origin.x,
        frame.origin.y,
        frame.size.width,
        frame.size.height,
    );
    // SAFETY: a fresh allocation initialized with a valid window.
    let filter = unsafe {
        SCContentFilter::initWithDesktopIndependentWindow(SCContentFilter::alloc(), &sc_window)
    };
    let scale = backing_scale(&frame, displays)
        // Off every known display: the scale ScreenCaptureKit renders it at.
        // SAFETY: plain property getter on a valid filter.
        .or_else(|| ScaleFactor::new(f64::from(unsafe { filter.pointPixelScale() })))
        .unwrap_or(ScaleFactor::ONE);
    let config = window_configuration(window_canvas_size(frame.size, scale));
    Ok(capture_image(&filter, &config, None))
}

/// The error for a window ScreenCaptureKit does not list: a withheld grant when
/// it hides every other app's window, otherwise a window that closed, moved to
/// another Space, or was minimized since it was listed.
fn missing_window(content: &SCShareableContent, window: WindowId) -> Error {
    check_not_withheld(content).err().unwrap_or_else(|| {
        Error::Platform(format!(
            "window {} is not on screen: it may have closed, been minimized, or moved to \
             another Space",
            window.0
        ))
    })
}

/// The backing scale of the display showing the largest part of `frame`, the
/// scale macOS renders the window at. Ties go to the earlier display; `None`
/// when `frame` is on no display.
fn backing_scale(frame: &LogicalRect, displays: &[DisplayInfo]) -> Option<ScaleFactor> {
    let mut best: Option<(f64, ScaleFactor)> = None;
    for display in displays {
        let Some(overlap) = display.logical_bounds.intersection(frame) else {
            continue;
        };
        let area = overlap.size.width * overlap.size.height;
        if best.is_none_or(|(best_area, _)| area > best_area) {
            best = Some((area, display.scale_factor));
        }
    }
    best.map(|(_, scale)| scale)
}

/// The output size for a window of `frame` points at `scale`: its native pixel
/// size plus [`SHADOW_MARGIN`] on every side.
fn window_canvas_size(frame: LogicalSize, scale: ScaleFactor) -> PhysicalSize {
    LogicalSize::new(
        frame.width + 2.0 * SHADOW_MARGIN,
        frame.height + 2.0 * SHADOW_MARGIN,
    )
    .to_physical(scale)
}

/// [`stream_configuration`] for a single window: shadow included, translucent
/// areas and rounded corners kept transparent, and the whole window captured even
/// where it is off screen. Every property used here exists since macOS 14.0, the
/// minimum Chartreuse supports.
fn window_configuration(size: PhysicalSize) -> Retained<SCStreamConfiguration> {
    let config = stream_configuration(size);
    // SAFETY: documented setters on a valid configuration.
    unsafe {
        config.setIgnoreShadowsSingleWindow(false);
        config.setShouldBeOpaque(false);
        config.setIgnoreGlobalClipSingleWindow(true);
    }
    config
}

/// Crops `image` to the smallest rectangle holding every pixel with non-zero
/// alpha: the window and its shadow, without the unused rest of the canvas.
/// `None` when every pixel is fully transparent.
fn trim_transparent_edges(image: &Image) -> Option<Image> {
    let size = image.size();
    let width = size.width as usize;
    if width == 0 {
        return None;
    }
    let visible = |row: &[u8]| {
        row.as_chunks::<4>()
            .0
            .iter()
            .position(|pixel| pixel[3] != 0)
    };
    let rows: Vec<&[u8]> = image.pixels().chunks_exact(width * 4).collect();
    let top = rows.iter().position(|row| visible(row).is_some())?;
    let bottom = rows.iter().rposition(|row| visible(row).is_some())?;
    let (mut left, mut right) = (width, 0);
    for row in &rows[top..=bottom] {
        if let Some(first) = visible(row) {
            left = left.min(first);
            let last = row
                .as_chunks::<4>()
                .0
                .iter()
                .rposition(|pixel| pixel[3] != 0)
                .unwrap_or(first);
            right = right.max(last);
        }
    }
    let mut pixels = Vec::with_capacity((right + 1 - left) * (bottom + 1 - top) * 4);
    for row in &rows[top..=bottom] {
        pixels.extend_from_slice(&row[left * 4..(right + 1) * 4]);
    }
    let cropped = PhysicalSize::new((right + 1 - left) as u32, (bottom + 1 - top) as u32);
    Image::new(cropped, pixels).ok()
}

// ---------------------------------------------------------------------------
// Withheld-contents heuristics
// ---------------------------------------------------------------------------

/// On-screen windows owned by other processes, as seen by two APIs at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ForeignWindows {
    /// Listed by `SCShareableContent`, which filters by the Screen Recording grant.
    shareable: usize,
    /// Listed by `CGWindowListCopyWindowInfo`, which does not.
    on_screen: usize,
}

impl ForeignWindows {
    /// Whether ScreenCaptureKit hides every other app's window that the window
    /// server shows, the signature of a revoked or no-longer-matching grant.
    fn withheld(self) -> bool {
        self.shareable == 0 && self.on_screen > 0
    }
}

/// Fails with [`Error::PermissionDenied`] when `content` hides every other app's
/// window that the window server shows ([`ForeignWindows::withheld`]).
pub(super) fn check_not_withheld(content: &SCShareableContent) -> Result<()> {
    let own_pid = own_pid();
    let windows = ForeignWindows {
        shareable: shareable_foreign_windows(content, own_pid),
        on_screen: on_screen_foreign_windows(own_pid),
    };
    if windows.withheld() {
        tracing::warn!(
            ?windows,
            "ScreenCaptureKit lists no windows of other apps; treating Screen Recording as withheld"
        );
        return Err(permission_denied());
    }
    Ok(())
}

/// Whether every image is entirely fully transparent, what a withheld capture
/// looks like. Opaque images of any color, including all black, are real
/// content. An empty list is not blank: there is nothing to judge.
fn all_blank<'a>(images: impl IntoIterator<Item = &'a Image>) -> bool {
    let mut any = false;
    for image in images {
        any = true;
        if !is_transparent(image) {
            return false;
        }
    }
    any
}

/// Whether every pixel of `image` has alpha 0.
fn is_transparent(image: &Image) -> bool {
    image
        .pixels()
        .as_chunks::<4>()
        .0
        .iter()
        .all(|pixel| pixel[3] == 0)
}

/// Counts the on-screen windows the window server reports for processes other
/// than `own_pid`. This API needs no permission; without one it only omits titles.
fn on_screen_foreign_windows(own_pid: i32) -> usize {
    let Some(list) =
        CGWindowListCopyWindowInfo(CGWindowListOption::OptionOnScreenOnly, kCGNullWindowID)
    else {
        return 0;
    };
    // SAFETY: CGWindowListCopyWindowInfo returns an array of dictionaries keyed by
    // CFString (the kCGWindow* keys).
    let list =
        unsafe { CFRetained::cast_unchecked::<CFArray<CFDictionary<CFString, CFType>>>(list) };
    // SAFETY: `kCGWindowOwnerPID` is an immutable CoreGraphics constant.
    let pid_key = unsafe { kCGWindowOwnerPID };
    list.iter()
        .filter(|window| {
            let pid = window
                .get(pid_key)
                .and_then(|pid| pid.downcast_ref::<CFNumber>().and_then(CFNumber::as_i32));
            pid.is_some_and(|pid| pid != own_pid)
        })
        .count()
}

/// Counts the windows in `content` owned by processes other than `own_pid`.
fn shareable_foreign_windows(content: &SCShareableContent, own_pid: i32) -> usize {
    // SAFETY: `windows` is a plain property getter on a valid object.
    unsafe { content.windows() }
        .iter()
        .filter(|window| {
            // SAFETY: plain property getters on a valid `SCWindow`.
            let owner = unsafe { window.owningApplication() };
            // SAFETY: as above, on a valid `SCRunningApplication`.
            owner.is_none_or(|owner| unsafe { owner.processID() } != own_pid)
        })
        .count()
}

// ---------------------------------------------------------------------------
// Shared ScreenCaptureKit helpers (display and window capture)
// ---------------------------------------------------------------------------

/// This process's id, in the type `SCRunningApplication::processID` uses.
pub(super) fn own_pid() -> i32 {
    // Process ids fit in `pid_t`; the kernel never hands out larger ones.
    process::id().cast_signed()
}

/// Chartreuse's own entry in the shareable applications, if it has windows.
fn own_application(content: &SCShareableContent) -> Option<Retained<SCRunningApplication>> {
    let own_pid = own_pid();
    // SAFETY: `applications` and `processID` are plain property getters on valid
    // objects.
    unsafe { content.applications() }
        .iter()
        .find(|app| unsafe { app.processID() } == own_pid)
}

/// Fetches the on-screen shareable content (desktop windows included) and runs
/// `f` with it on ScreenCaptureKit's completion queue, resolving to its result.
///
/// ScreenCaptureKit objects are not `Send`, so whatever needs them happens inside
/// `f`; `f` hands back `Send` data, such as pending [`capture_image`] futures.
pub(super) fn with_shareable_content<T, F>(f: F) -> BoxFuture<'static, Result<T>>
where
    T: Send + 'static,
    F: FnOnce(&SCShareableContent) -> Result<T> + Send + 'static,
{
    let (sender, receiver) = oneshot::channel();
    // The block is `Fn`, but ScreenCaptureKit calls it once; take the state out.
    let state = Mutex::new(Some((f, sender)));
    let handler = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            let Some((f, sender)) = state.lock().take() else {
                return;
            };
            // SAFETY: ScreenCaptureKit passes valid (or null) objects that live for
            // the duration of the call.
            let result = match unsafe { (content.as_ref(), error.as_ref()) } {
                (Some(content), _) => f(content),
                (None, Some(error)) => Err(error_from_ns(error)),
                (None, None) => Err(Error::Platform(
                    "ScreenCaptureKit returned neither shareable content nor an error".into(),
                )),
            };
            // The receiver is gone only if the caller dropped the future.
            let _ = sender.send(result);
        },
    );
    // SAFETY: the handler has the signature ScreenCaptureKit expects and is
    // retained by it until called.
    unsafe {
        SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
            false, true, &handler,
        );
    }
    receiver.map(flatten_canceled).boxed()
}

/// Settings shared by every capture: `size` output pixels, sRGB, no cursor, and
/// the display's full resolution.
fn stream_configuration(size: PhysicalSize) -> Retained<SCStreamConfiguration> {
    // SAFETY: a fresh configuration, configured through its documented setters
    // with in-range values.
    unsafe {
        let config = SCStreamConfiguration::new();
        config.setWidth(size.width as usize);
        config.setHeight(size.height as usize);
        config.setShowsCursor(false);
        config.setColorSpaceName(kCGColorSpaceSRGB);
        config.setCaptureResolution(SCCaptureResolutionType::Best);
        config
    }
}

/// Starts a screenshot of `filter` and resolves to it as an [`Image`] of `size`,
/// or of the captured image's own size when `size` is `None`.
fn capture_image(
    filter: &SCContentFilter,
    config: &SCStreamConfiguration,
    size: Option<PhysicalSize>,
) -> BoxFuture<'static, Result<Image>> {
    let (sender, receiver) = oneshot::channel();
    let sender = Mutex::new(Some(sender));
    let handler = RcBlock::new(move |image: *mut CGImage, error: *mut NSError| {
        let Some(sender) = sender.lock().take() else {
            return;
        };
        // SAFETY: ScreenCaptureKit passes valid (or null) objects that live for
        // the duration of the call.
        let result = match unsafe { (image.as_ref(), error.as_ref()) } {
            (Some(image), _) => match size {
                Some(size) => image_from_cg(image, size),
                None => cg_image_to_image(image),
            },
            (None, Some(error)) => Err(error_from_ns(error)),
            (None, None) => Err(Error::Platform(
                "ScreenCaptureKit returned neither an image nor an error".into(),
            )),
        };
        let _ = sender.send(result);
    });
    // SAFETY: valid filter and configuration; the handler has the expected
    // signature and is retained by ScreenCaptureKit until called.
    unsafe {
        SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(
            filter,
            config,
            Some(&handler),
        );
    }
    receiver.map(flatten_canceled).boxed()
}

/// Unwraps a oneshot result whose sender was dropped without answering.
fn flatten_canceled<T>(result: Result<Result<T>, oneshot::Canceled>) -> Result<T> {
    result.unwrap_or_else(|_| {
        Err(Error::Platform(
            "ScreenCaptureKit dropped its completion handler without calling it".into(),
        ))
    })
}

/// Maps a ScreenCaptureKit `NSError` to an [`Error`].
fn error_from_ns(error: &NSError) -> Error {
    // SAFETY: `SCStreamErrorDomain` is an immutable framework constant.
    let in_sck_domain = error
        .domain()
        .isEqualToString(unsafe { SCStreamErrorDomain });
    sck_error(
        in_sck_domain,
        error.code(),
        error.localizedDescription().to_string(),
    )
}

/// [`error_from_ns`] without the Objective-C: a declined TCC check
/// (`SCStreamErrorUserDeclined`) is a missing Screen Recording grant; anything
/// else is a platform failure.
fn sck_error(in_sck_domain: bool, code: NSInteger, description: String) -> Error {
    if in_sck_domain && code == SCStreamErrorCode::UserDeclined.0 {
        permission_denied()
    } else {
        Error::Platform(format!("ScreenCaptureKit: {description} (code {code})"))
    }
}

#[cfg(test)]
mod tests {
    use chartreuse_core::color::Rgba8;

    use super::*;

    fn display(id: u64) -> DisplayInfo {
        let scale_factor = ScaleFactor::new(2.0).unwrap();
        let logical_bounds = LogicalRect::new(0.0, 0.0, 100.0, 50.0);
        DisplayInfo {
            id: DisplayId(id),
            name: format!("Display {id}"),
            logical_bounds,
            pixel_size: logical_bounds.size.to_physical(scale_factor),
            scale_factor,
            is_primary: id == 1,
        }
    }

    #[test]
    fn displays_match_by_id_in_display_model_order() {
        let matched = match_displays(vec![display(1), display(7)], &[7, 3, 1]).unwrap();
        let pairs: Vec<(u64, usize)> = matched.iter().map(|(d, i)| (d.id.0, *i)).collect();
        assert_eq!(pairs, [(1, 2), (7, 0)]);
    }

    #[test]
    fn a_display_missing_from_screencapturekit_is_an_error() {
        let error = match_displays(vec![display(1), display(2)], &[1]).unwrap_err();
        assert!(
            matches!(&error, Error::Platform(message) if message.contains("display 2")),
            "{error:?}"
        );
    }

    #[test]
    fn hidden_foreign_windows_mean_withheld_contents() {
        let withheld = |shareable, on_screen| {
            ForeignWindows {
                shareable,
                on_screen,
            }
            .withheld()
        };
        assert!(withheld(0, 12));
        assert!(!withheld(3, 12));
        // Nothing on screen to hide (or the window server list failed): no evidence.
        assert!(!withheld(0, 0));
    }

    fn size(width: u32, height: u32) -> PhysicalSize {
        PhysicalSize { width, height }
    }

    #[test]
    fn blank_only_when_every_image_is_fully_transparent() {
        let transparent = Image::filled(size(4, 3), Rgba8::new(0, 0, 0, 0));
        let black = Image::filled(size(4, 3), Rgba8::new(0, 0, 0, 255));
        let wallpaper = Image::filled(size(4, 3), Rgba8::new(10, 90, 200, 255));
        let mut nearly_transparent = transparent.clone();
        nearly_transparent.set_pixel(3, 2, Rgba8::new(0, 0, 0, 1));

        assert!(all_blank([&transparent]));
        assert!(all_blank([&transparent, &transparent]));
        // Uniform opaque frames are real content: a full-screen video paused on
        // black, a black slide, or a solid wallpaper with the menu bar hidden.
        assert!(!all_blank([&black]));
        assert!(!all_blank([&wallpaper]));
        // One display with any opaque pixel is real content.
        assert!(!all_blank([&transparent, &black]));
        assert!(!all_blank([&nearly_transparent]));
        assert!(!all_blank(std::iter::empty()));
    }

    #[test]
    fn user_declined_is_a_screen_recording_denial() {
        let declined = SCStreamErrorCode::UserDeclined.0;
        assert!(matches!(
            sck_error(true, declined, "declined".into()),
            Error::PermissionDenied(Permission::ScreenRecording)
        ));
        // The same code from another domain means something else.
        assert!(matches!(
            sck_error(false, declined, "x".into()),
            Error::Platform(_)
        ));
        assert!(matches!(
            sck_error(true, SCStreamErrorCode::InternalError.0, "boom".into()),
            Error::Platform(message) if message.contains("boom")
        ));
    }

    fn display_at(id: u64, bounds: LogicalRect, scale: f64) -> DisplayInfo {
        let scale_factor = ScaleFactor::new(scale).unwrap();
        DisplayInfo {
            id: DisplayId(id),
            name: format!("Display {id}"),
            logical_bounds: bounds,
            pixel_size: bounds.size.to_physical(scale_factor),
            scale_factor,
            is_primary: id == 1,
        }
    }

    /// A 2× primary with a 1× display to its left, slightly higher.
    fn two_displays() -> Vec<DisplayInfo> {
        vec![
            display_at(1, LogicalRect::new(0.0, 0.0, 1512.0, 982.0), 2.0),
            display_at(2, LogicalRect::new(-1920.0, -200.0, 1920.0, 1080.0), 1.0),
        ]
    }

    #[test]
    fn a_window_renders_at_the_scale_of_the_display_showing_most_of_it() {
        let displays = two_displays();
        let scale = |x, y, w, h| {
            backing_scale(&LogicalRect::new(x, y, w, h), &displays).map(ScaleFactor::get)
        };
        assert_eq!(scale(100.0, 100.0, 800.0, 600.0), Some(2.0));
        assert_eq!(scale(-1500.0, -150.0, 800.0, 600.0), Some(1.0));
        // Spanning both displays: 300 points on the left one, 500 on the primary.
        assert_eq!(scale(-300.0, 100.0, 800.0, 600.0), Some(2.0));
        assert_eq!(scale(-500.0, 100.0, 800.0, 600.0), Some(1.0));
        // Equal halves: the earlier display wins, so the answer is stable.
        assert_eq!(scale(-400.0, 100.0, 800.0, 600.0), Some(2.0));
        // Below the left display and left of the primary: on no display.
        assert_eq!(scale(-900.0, 900.0, 300.0, 200.0), None);
    }

    #[test]
    fn the_canvas_holds_the_window_at_native_size_plus_the_shadow_margin() {
        let at = |w, h, scale| {
            window_canvas_size(LogicalSize::new(w, h), ScaleFactor::new(scale).unwrap())
        };
        // SHADOW_MARGIN (128 points) on each side, at the window's scale.
        assert_eq!(at(1488.0, 1640.0, 2.0), size(3488, 3792));
        assert_eq!(at(800.0, 600.0, 1.0), size(1056, 856));
        // Fractional scales round to whole pixels.
        assert_eq!(at(801.0, 601.0, 1.5), size(1586, 1286));
    }

    #[test]
    fn trimming_keeps_the_window_and_its_translucent_shadow() {
        let clear = Rgba8::new(0, 0, 0, 0);
        let window = Rgba8::new(40, 50, 60, 255);
        let shadow = Rgba8::new(0, 0, 0, 1);
        // A 10×8 canvas: the window at x 3..=5, y 2..=4, a faint shadow pixel
        // below-right of it at (7, 6), everything else clear.
        let canvas = Image::from_fn(size(10, 8), |x, y| match (x, y) {
            (3..=5, 2..=4) => window,
            (7, 6) => shadow,
            _ => clear,
        });
        let trimmed = trim_transparent_edges(&canvas).unwrap();
        assert_eq!(trimmed.size(), size(5, 5));
        assert_eq!(trimmed.pixel(0, 0), Some(window));
        assert_eq!(trimmed.pixel(2, 2), Some(window));
        assert_eq!(trimmed.pixel(4, 4), Some(shadow));
        assert_eq!(trimmed.pixel(4, 0), Some(clear));
    }

    #[test]
    fn trimming_leaves_tight_images_alone_and_rejects_empty_ones() {
        let opaque = Image::filled(size(4, 3), Rgba8::new(1, 2, 3, 255));
        assert_eq!(trim_transparent_edges(&opaque), Some(opaque));
        let transparent = Image::filled(size(4, 3), Rgba8::new(9, 9, 9, 0));
        assert_eq!(trim_transparent_edges(&transparent), None);
        let empty = Image::filled(size(0, 0), Rgba8::new(0, 0, 0, 0));
        assert_eq!(trim_transparent_edges(&empty), None);
    }
}

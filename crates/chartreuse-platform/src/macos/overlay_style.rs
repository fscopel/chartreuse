//! macOS: overlay window styling through the window's `NSWindow`.
//!
//! [`MacosOverlayStyle::apply`] raises the window to [`overlay_level`], above the
//! menu bar and Dock; makes it join every Space and appear over full-screen apps
//! ([`overlay_collection_behavior`]); keeps it visible when the app deactivates;
//! removes its shadow; snaps its frame to the screen it is on
//! ([`target_frame`]); and activates the app and makes the window key, so that
//! Escape reaches it even though Chartreuse is an accessory app that is usually
//! inactive when a capture starts.

use chartreuse_core::display::DisplayInfo;
use chartreuse_core::{Error, Result};
use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSApplication, NSScreen, NSScreenSaverWindowLevel, NSView, NSWindow,
    NSWindowCollectionBehavior, NSWindowLevel,
};
use objc2_core_foundation::CGRect;
use raw_window_handle::{RawWindowHandle, WindowHandle};

use crate::overlay_style::{NativeWindow, OverlayWindowStyle};

/// The macOS [`OverlayWindowStyle`] backend.
#[derive(Debug, Default)]
pub struct MacosOverlayStyle;

impl MacosOverlayStyle {
    pub fn new() -> Self {
        Self
    }
}

impl OverlayWindowStyle for MacosOverlayStyle {
    fn apply(&self, window: NativeWindow<'_>, _display: &DisplayInfo) -> Result<()> {
        let mtm = MainThreadMarker::new().ok_or_else(|| {
            Error::Platform("overlay windows must be styled on the main thread".into())
        })?;
        let window = ns_window(window.window, mtm)?;

        window.setLevel(overlay_level());
        window.setCollectionBehavior(overlay_collection_behavior(window.collectionBehavior()));
        window.setHidesOnDeactivate(false);
        window.setHasShadow(false);

        let frame = window.frame();
        let screens: Vec<CGRect> = NSScreen::screens(mtm)
            .iter()
            .map(|screen| screen.frame())
            .collect();
        let target = target_frame(frame, &screens).ok_or_else(|| {
            Error::Platform(format!("overlay window at {frame:?} is on no screen"))
        })?;
        if target != frame {
            tracing::debug!(?frame, ?target, "snapping overlay window to its screen");
            window.setFrame_display(target, true);
        }

        // An accessory app is normally inactive when a capture starts, and the key
        // window of an inactive app hears no keys. The cooperative `activate()`
        // (macOS 14) may decline when another app is frontmost, which is exactly
        // the case here, so this uses the ignoring variant, as winit does.
        #[allow(deprecated)]
        NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
        window.makeKeyAndOrderFront(None);
        if !window.isKeyWindow() {
            tracing::warn!("the overlay window did not become key; Escape may not reach it");
        }
        Ok(())
    }
}

/// The `NSWindow` behind an AppKit window handle.
fn ns_window(handle: WindowHandle<'_>, _mtm: MainThreadMarker) -> Result<Retained<NSWindow>> {
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return Err(Error::Platform(format!(
            "overlay window has a non-AppKit handle: {:?}",
            handle.as_raw()
        )));
    };
    // SAFETY: an AppKit window handle's `ns_view` points to a live `NSView` for as
    // long as the borrowed `WindowHandle` is alive, which outlives this borrow;
    // `_mtm` proves we are on the main thread, where AppKit views may be used.
    let view: &NSView = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    view.window()
        .ok_or_else(|| Error::Platform("overlay view is not in a window".into()))
}

/// The window level of an overlay: `NSScreenSaverWindowLevel` (1000).
///
/// An overlay shows the frozen screen, so it has to cover everything that was
/// visible when the capture was taken and could still be on screen: the Dock
/// (20), the menu bar (24) and its status items (25), and pop-up menus (101), such
/// as a menu that was open when the hotkey fired. The screen-saver level is the
/// lowest named level above all of those. Higher levels (up to
/// `kCGMaximumWindowLevel`) would also cover the cursor, the screen shield that
/// `CGDisplayCapture` uses, and system alerts such as the Screen Recording
/// prompt, which must stay reachable.
pub(crate) fn overlay_level() -> NSWindowLevel {
    NSScreenSaverWindowLevel
}

/// `current` with the overlay's Spaces, Mission Control, and window-cycling
/// behavior: on every Space (`CanJoinAllSpaces`), including over full-screen apps
/// (`FullScreenAuxiliary`), unaffected by Mission Control (`Stationary`), and
/// left out of Cmd-` cycling (`IgnoresCycle`).
///
/// AppKit rejects mutually exclusive flags, so the competing flag of each group
/// is cleared: `MoveToActiveSpace`; `Managed` and `Transient`;
/// `ParticipatesInCycle`; `FullScreenPrimary` and `FullScreenNone`. Unrelated
/// flags (such as the tiling ones) are kept.
pub(crate) fn overlay_collection_behavior(
    current: NSWindowCollectionBehavior,
) -> NSWindowCollectionBehavior {
    let competing = NSWindowCollectionBehavior::MoveToActiveSpace
        | NSWindowCollectionBehavior::Managed
        | NSWindowCollectionBehavior::Transient
        | NSWindowCollectionBehavior::ParticipatesInCycle
        | NSWindowCollectionBehavior::FullScreenPrimary
        | NSWindowCollectionBehavior::FullScreenNone;
    let overlay = NSWindowCollectionBehavior::CanJoinAllSpaces
        | NSWindowCollectionBehavior::FullScreenAuxiliary
        | NSWindowCollectionBehavior::Stationary
        | NSWindowCollectionBehavior::IgnoresCycle;
    current.difference(competing) | overlay
}

/// The frame an overlay window currently at `window` should have: the frame of
/// the screen it overlaps most, or `None` if it overlaps none. The first screen
/// wins a tie.
///
/// Everything is in AppKit's global screen coordinates (bottom-left origin at
/// the primary screen's bottom-left corner, y up), so no flip is involved: the
/// overlay was already placed on its display through the display model (winit
/// flips iced's top-left logical position against the primary screen's height,
/// the same flip as `displays::flip_frame`), and this only corrects any offset
/// or size AppKit or winit introduced on the way, without ever moving the
/// window to a different display.
pub(crate) fn target_frame(window: CGRect, screens: &[CGRect]) -> Option<CGRect> {
    let overlap = |screen: &CGRect| {
        let width = (window.origin.x + window.size.width).min(screen.origin.x + screen.size.width)
            - window.origin.x.max(screen.origin.x);
        let height = (window.origin.y + window.size.height)
            .min(screen.origin.y + screen.size.height)
            - window.origin.y.max(screen.origin.y);
        if width > 0.0 && height > 0.0 {
            width * height
        } else {
            0.0
        }
    };
    let mut best: Option<(f64, CGRect)> = None;
    for screen in screens {
        let area = overlap(screen);
        if area > 0.0 && best.is_none_or(|(most, _)| area > most) {
            best = Some((area, *screen));
        }
    }
    best.map(|(_, screen)| screen)
}

#[cfg(test)]
mod tests {
    use objc2_core_foundation::{CGPoint, CGSize};
    use objc2_core_graphics::{CGShieldingWindowLevel, CGWindowLevelForKey, CGWindowLevelKey};

    use super::*;

    type Behavior = NSWindowCollectionBehavior;

    fn rect(x: f64, y: f64, width: f64, height: f64) -> CGRect {
        CGRect::new(CGPoint::new(x, y), CGSize::new(width, height))
    }

    /// The window server's level for `key`.
    fn level_for(key: CGWindowLevelKey) -> NSWindowLevel {
        NSWindowLevel::try_from(CGWindowLevelForKey(key)).unwrap()
    }

    /// The fake backend's default desktop in AppKit coordinates: a 1512×982
    /// primary, a 1920×1080 display to its left whose top edge is 400 points above
    /// the primary's top, and an 800×1280 portrait display to its right whose top
    /// edge is 100 points below the primary's.
    fn screens() -> [CGRect; 3] {
        [
            rect(0.0, 0.0, 1512.0, 982.0),
            rect(-1920.0, 302.0, 1920.0, 1080.0),
            rect(1512.0, -398.0, 800.0, 1280.0),
        ]
    }

    #[test]
    fn overlays_sit_above_the_dock_menu_bar_status_items_and_pop_up_menus() {
        let level = overlay_level();
        for key in [
            CGWindowLevelKey::DockWindowLevelKey,
            CGWindowLevelKey::MainMenuWindowLevelKey,
            CGWindowLevelKey::StatusWindowLevelKey,
            CGWindowLevelKey::PopUpMenuWindowLevelKey,
        ] {
            let below = level_for(key);
            assert!(level > below, "{level} is not above {key:?} ({below})");
        }
    }

    #[test]
    fn overlays_stay_below_the_cursor_assistive_tech_and_the_display_shield() {
        let level = overlay_level();
        let shield = NSWindowLevel::try_from(CGShieldingWindowLevel()).unwrap();
        for (name, above) in [
            ("cursor", level_for(CGWindowLevelKey::CursorWindowLevelKey)),
            (
                "assistive tech",
                level_for(CGWindowLevelKey::AssistiveTechHighWindowLevelKey),
            ),
            ("display shield", shield),
        ] {
            assert!(
                level < above,
                "{level} is not below the {name} level ({above})"
            );
        }
    }

    #[test]
    fn overlay_behavior_joins_all_spaces_over_full_screen_apps() {
        assert_eq!(
            overlay_collection_behavior(Behavior::Default),
            Behavior::CanJoinAllSpaces
                | Behavior::FullScreenAuxiliary
                | Behavior::Stationary
                | Behavior::IgnoresCycle
        );
    }

    #[test]
    fn overlay_behavior_clears_the_competing_flag_of_each_group() {
        let competing = Behavior::MoveToActiveSpace
            | Behavior::Managed
            | Behavior::Transient
            | Behavior::ParticipatesInCycle
            | Behavior::FullScreenPrimary
            | Behavior::FullScreenNone;
        assert_eq!(
            overlay_collection_behavior(competing),
            overlay_collection_behavior(Behavior::Default)
        );
    }

    #[test]
    fn overlay_behavior_keeps_unrelated_flags() {
        let behavior = overlay_collection_behavior(Behavior::FullScreenDisallowsTiling);
        assert!(behavior.contains(Behavior::FullScreenDisallowsTiling));
        assert!(behavior.contains(Behavior::CanJoinAllSpaces));
    }

    #[test]
    fn a_window_placed_exactly_keeps_its_frame() {
        for screen in screens() {
            assert_eq!(target_frame(screen, &screens()), Some(screen));
        }
    }

    #[test]
    fn a_window_pushed_below_the_menu_bar_snaps_back_over_it() {
        // AppKit keeps titled windows out of the menu bar: on the primary, that is
        // the top edge 24 points lower.
        let pushed = rect(0.0, 0.0, 1512.0, 958.0);
        assert_eq!(target_frame(pushed, &screens()), Some(screens()[0]));
    }

    #[test]
    fn a_window_straddling_two_screens_goes_to_the_one_it_overlaps_most() {
        // Mostly on the left display, partly on the primary.
        let straddling = rect(-1500.0, 400.0, 1920.0, 1080.0);
        assert_eq!(target_frame(straddling, &screens()), Some(screens()[1]));
    }

    #[test]
    fn a_window_on_no_screen_has_no_target() {
        assert_eq!(
            target_frame(rect(5000.0, 5000.0, 100.0, 100.0), &screens()),
            None
        );
        // Touching an edge is not overlapping.
        assert_eq!(
            target_frame(rect(0.0, 982.0, 100.0, 100.0), &screens()[..1]),
            None
        );
    }
}

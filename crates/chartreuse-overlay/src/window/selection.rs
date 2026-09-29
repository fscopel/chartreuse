//! The window-selection state machine, in global logical desktop coordinates.

use chartreuse_core::display::DisplayLayout;
use chartreuse_core::geometry::LogicalPoint;
use chartreuse_core::window::{topmost_at, WindowId, WindowInfo};

/// Where a [`WindowSelection`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Following the pointer; a click on a window commits it.
    Selecting,
    /// Finished with this window. Terminal: later input is ignored.
    Committed(WindowId),
    /// Cancelled with Escape. Terminal: later input is ignored.
    Cancelled,
}

/// Pointer and keyboard input for a [`WindowSelection`], in global logical
/// coordinates. The overlay canvases produce these (see
/// [`WindowOverlay`](super::WindowOverlay)).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Input {
    /// The pointer moved to this point.
    Move(LogicalPoint),
    /// The primary button was pressed and released; this is where it was
    /// released.
    Click(LogicalPoint),
    /// Escape was pressed.
    Escape,
    /// Space was pressed, with the pointer at this point if it is over the
    /// overlay. Not part of the selection, which ignores it: the app may switch
    /// to another kind of selection with it.
    Space(Option<LogicalPoint>),
}

/// How a window selection ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The user clicked this window. Capture it with
    /// `Capture::capture_window`; [`WindowSelection::hovered`] still describes it.
    Commit(WindowId),
    /// The user pressed Escape.
    Cancel,
}

/// The window-selection gesture shared by every display's overlay.
///
/// One `WindowSelection` spans the whole desktop: the app owns it, routes every
/// overlay's [`Input`] to [`WindowSelection::apply`], and each display's canvas
/// leaves [`WindowSelection::hovered`]'s bounds undimmed.
///
/// The hovered window is the frontmost window under the pointer
/// ([`topmost_at`]), but only where the pointer is on a display: a point in a
/// gap between displays, or off the desktop, hovers nothing even if a window's
/// bounds reach it, because nothing there can be seen.
///
/// Transitions:
///
/// - **Move** records the pointer and re-resolves the hovered window.
/// - **Click** does the same at the click point, then commits the hovered
///   window. A click where no window is hovered (on the bare desktop, or in a
///   gap) is ignored, so the user can simply click again or press Escape.
/// - **Escape** cancels.
/// - **Space** is ignored.
///
/// Once committed or cancelled, later input is ignored and the pointer and
/// hovered window stay as they were, so the overlays keep showing the committed
/// window until the app closes them.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowSelection {
    layout: DisplayLayout,
    windows: Vec<WindowInfo>,
    pointer: Option<LogicalPoint>,
    /// Index into `windows`.
    hovered: Option<usize>,
    phase: Phase,
}

impl WindowSelection {
    /// A selection among `windows` (the platform's window list, captured with the
    /// displays in `layout`), with nothing hovered until the pointer moves; to
    /// highlight the window under the pointer at once, apply an [`Input::Move`]
    /// with its current position.
    #[must_use]
    pub const fn new(layout: DisplayLayout, windows: Vec<WindowInfo>) -> Self {
        Self {
            layout,
            windows,
            pointer: None,
            hovered: None,
            phase: Phase::Selecting,
        }
    }

    /// The displays the selection spans.
    #[must_use]
    pub const fn layout(&self) -> &DisplayLayout {
        &self.layout
    }

    /// The windows to choose from.
    #[must_use]
    pub fn windows(&self) -> &[WindowInfo] {
        &self.windows
    }

    /// Where the selection is; see [`Phase`].
    #[must_use]
    pub const fn phase(&self) -> Phase {
        self.phase
    }

    /// The pointer's latest position, once it has moved or clicked.
    #[must_use]
    pub const fn pointer(&self) -> Option<LogicalPoint> {
        self.pointer
    }

    /// The window under the pointer; after a commit, the committed window.
    #[must_use]
    pub fn hovered(&self) -> Option<&WindowInfo> {
        self.hovered.map(|index| &self.windows[index])
    }

    /// The window a pointer at `point` would hover.
    #[must_use]
    pub fn window_at(&self, point: LogicalPoint) -> Option<&WindowInfo> {
        self.index_at(point).map(|index| &self.windows[index])
    }

    /// Feeds one input to the state machine, returning the outcome if it ended
    /// the selection.
    pub fn apply(&mut self, input: Input) -> Option<Outcome> {
        match input {
            Input::Move(point) => {
                self.move_to(point);
                None
            }
            Input::Click(point) => self.click(point),
            Input::Escape => self.escape(),
            Input::Space(_) => None,
        }
    }

    /// The pointer moved to `point`.
    pub fn move_to(&mut self, point: LogicalPoint) {
        if self.is_terminal() {
            return;
        }
        self.pointer = Some(point);
        self.hovered = self.index_at(point);
    }

    /// The primary button was clicked at `point`.
    pub fn click(&mut self, point: LogicalPoint) -> Option<Outcome> {
        if self.is_terminal() {
            return None;
        }
        self.move_to(point);
        let id = self.hovered()?.id;
        self.phase = Phase::Committed(id);
        Some(Outcome::Commit(id))
    }

    /// Escape was pressed.
    pub fn escape(&mut self) -> Option<Outcome> {
        if self.is_terminal() {
            return None;
        }
        self.phase = Phase::Cancelled;
        Some(Outcome::Cancel)
    }

    const fn is_terminal(&self) -> bool {
        !matches!(self.phase, Phase::Selecting)
    }

    fn index_at(&self, point: LogicalPoint) -> Option<usize> {
        self.layout.display_at(point)?;
        let window = topmost_at(&self.windows, point)?;
        self.windows
            .iter()
            .position(|candidate| std::ptr::eq(candidate, window))
    }
}

#[cfg(test)]
mod tests {
    use chartreuse_core::geometry::LogicalRect;
    use chartreuse_core::window::WindowOwner;
    use chartreuse_platform::fake::{default_displays, default_windows};

    use super::*;

    fn layout() -> DisplayLayout {
        DisplayLayout::new(default_displays()).expect("the fake displays form a layout")
    }

    /// The fake desktop's windows: 101 on the primary display, in front of 102,
    /// which spans the external and primary displays; 103 on the portrait; 104
    /// on the external display.
    fn selection() -> WindowSelection {
        WindowSelection::new(layout(), default_windows())
    }

    fn pt(x: f64, y: f64) -> LogicalPoint {
        LogicalPoint::new(x, y)
    }

    fn hovered(selection: &WindowSelection) -> Option<WindowId> {
        selection.hovered().map(|window| window.id)
    }

    fn window(id: u64, z_order: u32, bounds: LogicalRect) -> WindowInfo {
        WindowInfo {
            id: WindowId(id),
            title: None,
            owner: WindowOwner {
                name: "Test".into(),
                pid: None,
            },
            bounds,
            z_order,
        }
    }

    #[test]
    fn the_frontmost_window_under_the_pointer_is_hovered() {
        let mut selection = selection();
        assert_eq!(hovered(&selection), None, "nothing until the pointer moves");
        for (point, expected) in [
            // 101 (z 0) overlaps 102 (z 1) on the primary display.
            (pt(300.0, 300.0), Some(101)),
            // Only 102 reaches left of 101.
            (pt(50.0, 300.0), Some(102)),
            // 102 on the external display, at negative x.
            (pt(-300.0, 100.0), Some(102)),
            // 104 alone, at negative x and y.
            (pt(-1700.0, -200.0), Some(104)),
            // 103 on the portrait display.
            (pt(1700.0, 400.0), Some(103)),
            // Bare desktop.
            (pt(1000.0, 900.0), None),
        ] {
            selection.move_to(point);
            assert_eq!(hovered(&selection), expected.map(WindowId), "at {point:?}");
            assert_eq!(selection.pointer(), Some(point));
        }
    }

    #[test]
    fn list_order_does_not_override_z_order() {
        // Listed back to front: z-order still decides.
        let windows = vec![
            window(1, 1, LogicalRect::new(0.0, 0.0, 500.0, 500.0)),
            window(2, 0, LogicalRect::new(100.0, 100.0, 100.0, 100.0)),
        ];
        let mut selection = WindowSelection::new(layout(), windows);
        selection.move_to(pt(150.0, 150.0));
        assert_eq!(hovered(&selection), Some(WindowId(2)));
        selection.move_to(pt(50.0, 50.0));
        assert_eq!(hovered(&selection), Some(WindowId(1)));
    }

    #[test]
    fn a_window_spanning_displays_is_hovered_from_either() {
        let mut selection = selection();
        // 102 spans x = -600..600: the external display left of 0, the primary
        // right of it. Display bounds are half-open, so x = 0 is on the primary.
        for point in [pt(-599.0, 60.0), pt(-0.5, 400.0), pt(0.0, 400.0)] {
            selection.move_to(point);
            assert_eq!(hovered(&selection), Some(WindowId(102)), "at {point:?}");
        }
    }

    #[test]
    fn the_pointer_in_a_gap_between_displays_hovers_nothing() {
        let mut selection = selection();
        // 102 reaches down to y = 750, but left of x = 0 the external display
        // ends at y = 680: (-100, 700) is in 102's bounds yet on no display.
        assert!(selection
            .windows()
            .iter()
            .any(|w| w.id == WindowId(102) && w.bounds.contains(pt(-100.0, 700.0))));
        selection.move_to(pt(-100.0, 300.0));
        assert_eq!(hovered(&selection), Some(WindowId(102)));
        selection.move_to(pt(-100.0, 700.0));
        assert_eq!(hovered(&selection), None);
        // Off the desktop altogether.
        selection.move_to(pt(5000.0, 5000.0));
        assert_eq!(hovered(&selection), None);
    }

    #[test]
    fn a_click_on_a_window_commits_it() {
        let mut selection = selection();
        selection.move_to(pt(1000.0, 900.0));
        // The click's own point decides, even without a move there first.
        assert_eq!(
            selection.apply(Input::Click(pt(-300.0, 100.0))),
            Some(Outcome::Commit(WindowId(102)))
        );
        assert_eq!(selection.phase(), Phase::Committed(WindowId(102)));
        assert_eq!(hovered(&selection), Some(WindowId(102)));
    }

    #[test]
    fn a_click_on_no_window_is_ignored() {
        let mut selection = selection();
        for point in [pt(1000.0, 900.0), pt(-100.0, 700.0)] {
            assert_eq!(selection.apply(Input::Click(point)), None);
            assert_eq!(selection.phase(), Phase::Selecting);
            assert_eq!(hovered(&selection), None);
        }
        // The user can go on to pick a window.
        assert_eq!(
            selection.apply(Input::Click(pt(300.0, 300.0))),
            Some(Outcome::Commit(WindowId(101)))
        );
    }

    #[test]
    fn escape_cancels_even_over_a_window() {
        let mut selection = selection();
        selection.move_to(pt(300.0, 300.0));
        assert_eq!(selection.apply(Input::Escape), Some(Outcome::Cancel));
        assert_eq!(selection.phase(), Phase::Cancelled);
    }

    #[test]
    fn input_after_the_end_is_ignored() {
        let mut committed = selection();
        committed.click(pt(300.0, 300.0));
        committed.move_to(pt(1700.0, 400.0));
        assert_eq!(hovered(&committed), Some(WindowId(101)), "keeps the commit");
        assert_eq!(committed.pointer(), Some(pt(300.0, 300.0)));
        assert_eq!(committed.apply(Input::Escape), None);
        assert_eq!(committed.apply(Input::Click(pt(1700.0, 400.0))), None);
        assert_eq!(committed.phase(), Phase::Committed(WindowId(101)));

        let mut cancelled = selection();
        cancelled.escape();
        assert_eq!(cancelled.apply(Input::Click(pt(300.0, 300.0))), None);
        assert_eq!(cancelled.apply(Input::Escape), None);
        assert_eq!(cancelled.phase(), Phase::Cancelled);
    }
}

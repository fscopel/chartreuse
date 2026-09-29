//! The selection state machine, in global logical desktop coordinates.

use chartreuse_core::display::{DisplayLayout, PixelGrid};
use chartreuse_core::geometry::{LogicalPoint, LogicalRect};

/// How far (in logical points, along either axis) the pointer must move from
/// where it was pressed before the press becomes a drag. Smaller wobbles are
/// part of a click.
pub const DRAG_THRESHOLD: f64 = 3.0;

/// Where a [`Selection`] is in its gesture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Phase {
    /// Waiting for the pointer to be pressed.
    Idle,
    /// Pressed at `anchor`, not yet moved past [`DRAG_THRESHOLD`].
    Pressed { anchor: LogicalPoint },
    /// Dragging from `anchor` to `current`; the live rectangle spans the two.
    Dragging {
        anchor: LogicalPoint,
        current: LogicalPoint,
    },
    /// Finished with this rectangle. Terminal: later input is ignored.
    Committed(LogicalRect),
    /// Cancelled with Escape. Terminal: later input is ignored.
    Cancelled,
}

/// Pointer and keyboard input for a [`Selection`], in global logical
/// coordinates. The overlay canvases produce these (see
/// [`RectangleOverlay`](super::RectangleOverlay)).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Input {
    /// The primary button went down at this point.
    Press(LogicalPoint),
    /// The pointer moved to this point while the button is down.
    Move(LogicalPoint),
    /// The primary button went up at this point.
    Release(LogicalPoint),
    /// Escape was pressed.
    Escape,
    /// Space was pressed, with the pointer at this point if it is over the
    /// overlay. Not part of the gesture, which ignores it: the app may switch
    /// to another kind of selection with it.
    Space(Option<LogicalPoint>),
}

/// How a selection ended.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Outcome {
    /// The user selected this rectangle (global logical coordinates, clamped to
    /// the desktop, on at least one display, and at least one output pixel in
    /// each dimension). Crop it with [`output_grid`].
    Commit(LogicalRect),
    /// The user pressed Escape.
    Cancel,
}

/// The rectangle-selection gesture shared by every display's overlay.
///
/// One `Selection` spans the whole desktop: the app owns it, routes every
/// overlay's [`Input`] to [`Selection::apply`], and each display's canvas draws
/// its slice of [`Selection::rect`].
///
/// Transitions:
///
/// - **Press** starts a gesture at the pressed point (from any non-terminal
///   phase; a press during a gesture, whose release was lost, restarts it).
/// - **Move** while pressed becomes a drag once the pointer is
///   [`DRAG_THRESHOLD`] away from the anchor; while dragging it updates the live
///   rectangle. Moves while idle are ignored.
/// - **Release** while dragging commits the rectangle, unless it selects
///   nothing: a rectangle lying entirely off the displays (in the gaps between
///   them) or less than one output pixel wide or tall goes back to
///   [`Phase::Idle`]. A release without a drag (a click) also goes back to idle,
///   so the user can simply try again.
/// - **Escape** cancels from any non-terminal phase, mid-drag included.
/// - **Space** is ignored.
///
/// Every point is clamped to the desktop's bounding rectangle
/// ([`DisplayLayout::bounds`]) first, so dragging past the edge of the desktop
/// pins the rectangle to that edge.
#[derive(Debug, Clone, PartialEq)]
pub struct Selection {
    layout: DisplayLayout,
    phase: Phase,
}

impl Selection {
    /// An idle selection over `layout`'s desktop.
    #[must_use]
    pub const fn new(layout: DisplayLayout) -> Self {
        Self {
            layout,
            phase: Phase::Idle,
        }
    }

    /// The displays the selection spans.
    #[must_use]
    pub const fn layout(&self) -> &DisplayLayout {
        &self.layout
    }
    /// Where the gesture is; see [`Phase`].
    #[must_use]
    pub const fn phase(&self) -> Phase {
        self.phase
    }

    /// True while the button is down (pressed or dragging).
    #[must_use]
    pub const fn is_active(&self) -> bool {
        matches!(self.phase, Phase::Pressed { .. } | Phase::Dragging { .. })
    }

    /// The pointer's latest position during a gesture: the anchor while pressed,
    /// the dragged corner while dragging.
    #[must_use]
    pub const fn pointer(&self) -> Option<LogicalPoint> {
        match self.phase {
            Phase::Pressed { anchor } => Some(anchor),
            Phase::Dragging { current, .. } => Some(current),
            Phase::Idle | Phase::Committed(_) | Phase::Cancelled => None,
        }
    }

    /// The rectangle to show: the live rectangle while dragging, the final one
    /// once committed.
    #[must_use]
    pub fn rect(&self) -> Option<LogicalRect> {
        match self.phase {
            Phase::Dragging { anchor, current } => Some(LogicalRect::from_corners(anchor, current)),
            Phase::Committed(rect) => Some(rect),
            Phase::Idle | Phase::Pressed { .. } | Phase::Cancelled => None,
        }
    }

    /// Feeds one input to the state machine, returning the outcome if it ended
    /// the gesture.
    pub fn apply(&mut self, input: Input) -> Option<Outcome> {
        match input {
            Input::Press(point) => {
                self.press(point);
                None
            }
            Input::Move(point) => {
                self.move_to(point);
                None
            }
            Input::Release(point) => self.release(point),
            Input::Escape => self.escape(),
            Input::Space(_) => None,
        }
    }

    /// The primary button went down at `point`.
    pub fn press(&mut self, point: LogicalPoint) {
        if self.is_terminal() {
            return;
        }
        self.phase = Phase::Pressed {
            anchor: self.clamp(point),
        };
    }

    /// The pointer moved to `point`.
    pub fn move_to(&mut self, point: LogicalPoint) {
        let current = self.clamp(point);
        self.phase = match self.phase {
            Phase::Pressed { anchor } if is_drag(anchor, current) => {
                Phase::Dragging { anchor, current }
            }
            Phase::Dragging { anchor, .. } => Phase::Dragging { anchor, current },
            phase => phase,
        };
    }

    /// The primary button went up at `point`.
    pub fn release(&mut self, point: LogicalPoint) -> Option<Outcome> {
        match self.phase {
            Phase::Pressed { .. } => {
                self.phase = Phase::Idle;
                None
            }
            Phase::Dragging { anchor, .. } => {
                let rect = LogicalRect::from_corners(anchor, self.clamp(point));
                if output_grid(&self.layout, &rect).is_some() {
                    self.phase = Phase::Committed(rect);
                    Some(Outcome::Commit(rect))
                } else {
                    self.phase = Phase::Idle;
                    None
                }
            }
            Phase::Idle | Phase::Committed(_) | Phase::Cancelled => None,
        }
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
        matches!(self.phase, Phase::Committed(_) | Phase::Cancelled)
    }

    fn clamp(&self, point: LogicalPoint) -> LogicalPoint {
        let bounds = self.layout.bounds();
        LogicalPoint::new(
            point.x.clamp(bounds.min_x(), bounds.max_x()),
            point.y.clamp(bounds.min_y(), bounds.max_y()),
        )
    }
}

fn is_drag(anchor: LogicalPoint, current: LogicalPoint) -> bool {
    (current.x - anchor.x).abs() >= DRAG_THRESHOLD || (current.y - anchor.y).abs() >= DRAG_THRESHOLD
}

/// The pixel grid to crop a selection onto: `rect` at the **largest** scale
/// factor among the displays it covers, so no display loses detail and
/// lower-scale content is upscaled (the project-wide mixed-scale rule; see
/// [`chartreuse_core::display`]). Its [`PixelGrid::pixel_size`] is the size of
/// the output image; pass the grid to `chartreuse_imaging::composite_at` with the
/// frozen captures to crop.
///
/// `None` if `rect` covers no display, or rounds to zero output pixels in either
/// dimension; such a rectangle selects nothing.
#[must_use]
pub fn output_grid(layout: &DisplayLayout, rect: &LogicalRect) -> Option<PixelGrid> {
    layout.capture_grid(rect).filter(|grid| {
        let size = grid.pixel_size();
        size.width > 0 && size.height > 0
    })
}

#[cfg(test)]
mod tests {
    use chartreuse_core::display::DisplayId;
    use chartreuse_core::geometry::{PhysicalSize, ScaleFactor};
    use chartreuse_platform::fake::default_displays;

    use super::*;

    fn layout() -> DisplayLayout {
        DisplayLayout::new(default_displays()).expect("the fake displays form a layout")
    }

    fn selection() -> Selection {
        Selection::new(layout())
    }

    fn pt(x: f64, y: f64) -> LogicalPoint {
        LogicalPoint::new(x, y)
    }

    /// Presses at `from`, drags through `to`, and releases there.
    fn drag(selection: &mut Selection, from: LogicalPoint, to: LogicalPoint) -> Option<Outcome> {
        selection.apply(Input::Press(from));
        selection.apply(Input::Move(to));
        selection.apply(Input::Release(to))
    }

    #[test]
    fn a_drag_shows_a_live_rect_and_commits_it_normalized() {
        let mut selection = selection();
        selection.press(pt(500.0, 400.0));
        assert_eq!(selection.rect(), None);
        selection.move_to(pt(200.0, 300.0));
        assert_eq!(
            selection.rect(),
            Some(LogicalRect::new(200.0, 300.0, 300.0, 100.0))
        );
        let outcome = selection.release(pt(100.0, 250.0));
        let rect = LogicalRect::new(100.0, 250.0, 400.0, 150.0);
        assert_eq!(outcome, Some(Outcome::Commit(rect)));
        assert_eq!(selection.phase(), Phase::Committed(rect));
        assert_eq!(selection.rect(), Some(rect));
    }

    #[test]
    fn a_drag_across_displays_keeps_global_coordinates() {
        // From the 1× external display (negative origin), across the 2× primary,
        // onto the 1.5× portrait display.
        let mut selection = selection();
        let outcome = drag(&mut selection, pt(-300.0, -100.0), pt(1700.0, 500.0));
        assert_eq!(
            outcome,
            Some(Outcome::Commit(LogicalRect::new(
                -300.0, -100.0, 2000.0, 600.0
            )))
        );
    }

    #[test]
    fn a_drag_entirely_at_negative_coordinates_commits() {
        let mut selection = selection();
        let outcome = drag(&mut selection, pt(-100.0, -50.0), pt(-900.0, -350.0));
        assert_eq!(
            outcome,
            Some(Outcome::Commit(LogicalRect::new(
                -900.0, -350.0, 800.0, 300.0
            )))
        );
    }

    #[test]
    fn escape_mid_drag_cancels_and_ends_the_gesture() {
        let mut selection = selection();
        selection.press(pt(10.0, 10.0));
        selection.move_to(pt(200.0, 200.0));
        assert_eq!(selection.apply(Input::Escape), Some(Outcome::Cancel));
        assert_eq!(selection.phase(), Phase::Cancelled);
        assert_eq!(selection.rect(), None);
        // Terminal: the release that follows (and anything else) is ignored.
        assert_eq!(selection.apply(Input::Release(pt(200.0, 200.0))), None);
        selection.press(pt(50.0, 50.0));
        assert_eq!(selection.escape(), None);
        assert_eq!(selection.phase(), Phase::Cancelled);
    }

    #[test]
    fn escape_cancels_before_any_press() {
        let mut selection = selection();
        assert_eq!(selection.escape(), Some(Outcome::Cancel));
    }

    #[test]
    fn a_click_without_a_drag_returns_to_idle() {
        let mut selection = selection();
        selection.press(pt(100.0, 100.0));
        // A wobble smaller than the threshold is still a click.
        selection.move_to(pt(100.0 + DRAG_THRESHOLD - 0.5, 101.0));
        assert_eq!(
            selection.phase(),
            Phase::Pressed {
                anchor: pt(100.0, 100.0)
            }
        );
        assert_eq!(selection.release(pt(102.0, 101.0)), None);
        assert_eq!(selection.phase(), Phase::Idle);
        // ...and the user can try again.
        let outcome = drag(&mut selection, pt(100.0, 100.0), pt(150.0, 120.0));
        assert!(matches!(outcome, Some(Outcome::Commit(_))));
    }

    #[test]
    fn moving_the_threshold_along_one_axis_starts_a_drag() {
        let mut selection = selection();
        selection.press(pt(100.0, 100.0));
        selection.move_to(pt(100.0, 100.0 - DRAG_THRESHOLD));
        assert!(matches!(selection.phase(), Phase::Dragging { .. }));
    }

    #[test]
    fn a_drag_released_level_with_the_anchor_selects_nothing() {
        let mut selection = selection();
        selection.press(pt(100.0, 100.0));
        selection.move_to(pt(300.0, 300.0));
        // A zero-height rectangle has no output pixels.
        assert_eq!(selection.release(pt(300.0, 100.0)), None);
        assert_eq!(selection.phase(), Phase::Idle);
    }

    #[test]
    fn a_drag_in_a_gap_between_displays_selects_nothing() {
        // Below the external display (which ends at y = 680) and left of the
        // primary: inside the desktop bounds, on no display.
        let mut selection = selection();
        assert_eq!(
            drag(&mut selection, pt(-500.0, 700.0), pt(-100.0, 900.0)),
            None
        );
        assert_eq!(selection.phase(), Phase::Idle);
    }

    #[test]
    fn moves_while_idle_are_ignored() {
        let mut selection = selection();
        selection.move_to(pt(10.0, 10.0));
        assert_eq!(selection.phase(), Phase::Idle);
        assert_eq!(selection.release(pt(10.0, 10.0)), None);
        assert_eq!(selection.phase(), Phase::Idle);
    }

    #[test]
    fn a_press_during_a_gesture_restarts_it() {
        let mut selection = selection();
        selection.press(pt(10.0, 10.0));
        selection.move_to(pt(400.0, 400.0));
        selection.press(pt(600.0, 600.0));
        assert_eq!(
            selection.phase(),
            Phase::Pressed {
                anchor: pt(600.0, 600.0)
            }
        );
    }

    #[test]
    fn points_are_clamped_to_the_desktop_bounds() {
        // The fake desktop spans x -1920..2312 and y -400..1380.
        let mut selection = selection();
        selection.press(pt(-5000.0, -5000.0));
        assert_eq!(selection.pointer(), Some(pt(-1920.0, -400.0)));
        let outcome = selection.apply(Input::Move(pt(-1800.0, 9000.0)));
        assert_eq!(outcome, None);
        assert_eq!(selection.pointer(), Some(pt(-1800.0, 1380.0)));
        // Released far off the right edge: pinned there, not at the release point.
        let outcome = selection.release(pt(9000.0, 0.0));
        assert_eq!(
            outcome,
            Some(Outcome::Commit(LogicalRect::new(
                -1920.0, -400.0, 4232.0, 400.0
            )))
        );
    }

    #[test]
    fn dragging_past_an_edge_along_it_selects_nothing() {
        // Both corners beyond the top edge clamp onto it: zero height.
        let mut selection = selection();
        assert_eq!(
            drag(&mut selection, pt(-1000.0, -900.0), pt(-500.0, -700.0)),
            None
        );
    }

    #[test]
    fn output_uses_the_largest_scale_among_covered_displays() {
        let layout = layout();
        let size = |rect: LogicalRect| output_grid(&layout, &rect).map(|grid| grid.pixel_size());

        // One display: its own scale.
        assert_eq!(
            size(LogicalRect::new(-1000.0, -300.0, 100.0, 50.0)),
            Some(PhysicalSize::new(100, 50))
        );
        assert_eq!(
            size(LogicalRect::new(1600.0, 200.0, 100.0, 50.0)),
            Some(PhysicalSize::new(150, 75))
        );
        // 1× external + 2× primary: 2×, the external half is upscaled.
        assert_eq!(
            size(LogicalRect::new(-100.0, 0.0, 300.0, 50.0)),
            Some(PhysicalSize::new(600, 100))
        );
        // 2× primary + 1.5× portrait: 2×.
        assert_eq!(
            size(LogicalRect::new(1400.0, 200.0, 200.0, 100.0)),
            Some(PhysicalSize::new(400, 200))
        );
        // A strip below the primary (which ends at y = 982) touches only the
        // 1.5× portrait display; the part over the gap still counts in the size.
        assert_eq!(
            size(LogicalRect::new(-100.0, 1000.0, 1700.0, 10.0)),
            Some(PhysicalSize::new(2550, 15))
        );
        // All three displays: 2×.
        let all = LogicalRect::new(-100.0, 600.0, 1700.0, 500.0);
        let grid = output_grid(&layout, &all).expect("covers displays");
        assert_eq!(grid.scale(), ScaleFactor::new(2.0).expect("valid"));
        assert_eq!(grid.pixel_size(), PhysicalSize::new(3400, 1000));
    }

    #[test]
    fn output_scale_ignores_displays_the_rect_only_borders() {
        // Ends exactly at the primary's left edge (x = 0): external display only.
        let layout = layout();
        let grid = output_grid(&layout, &LogicalRect::new(-200.0, 0.0, 200.0, 100.0))
            .expect("on the external display");
        assert_eq!(grid.scale(), ScaleFactor::ONE);
        assert_eq!(grid.pixel_size(), PhysicalSize::new(200, 100));
        let display = layout.get(DisplayId(2)).expect("external display");
        assert_eq!(display.scale_factor, grid.scale());
    }

    #[test]
    fn output_grid_rejects_rects_with_no_pixels() {
        let layout = layout();
        assert_eq!(
            output_grid(&layout, &LogicalRect::new(10.0, 10.0, 0.0, 50.0)),
            None
        );
        // 0.2 pt on a 2× display rounds to 0 pixels.
        assert_eq!(
            output_grid(&layout, &LogicalRect::new(10.0, 10.0, 0.2, 50.0)),
            None
        );
        assert_eq!(
            output_grid(&layout, &LogicalRect::new(10.0, 10.0, 0.25, 50.0))
                .map(|grid| grid.pixel_size()),
            Some(PhysicalSize::new(1, 100))
        );
    }
}

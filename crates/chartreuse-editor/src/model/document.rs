//! The document: base image, annotations in z-order, and the selection.

use std::collections::BTreeSet;
use std::sync::Arc;

use chartreuse_core::geometry::{PhysicalRect, PhysicalSize};
use chartreuse_core::image::Image;

use super::annotation::{Annotation, AnnotationId, Shape};
use super::geometry::{Point, Rect, Size};
use super::history::{Command, Edit, History, State};
use super::style::Style;

/// An image being annotated.
///
/// Annotations are kept in z-order: index 0 is drawn first (bottom), the last
/// is drawn on top. Annotations are flattened onto a copy of the base image
/// only at export. The base image changes only when the document is
/// [resized](Self::resize), and is then resampled from the image the
/// document was opened with, so resizing again loses nothing more.
///
/// The selection is a set of ids of annotations in the document; ids that stop
/// existing are dropped from it.
///
/// # Undo
///
/// [`add`](Self::add) and [`apply`](Self::apply) record one undo step each,
/// unless they change nothing; recording clears the redo stack. Selection
/// changes and [`set_text_size`](Self::set_text_size) are not edits and are
/// not recorded.
///
/// Selection across edits: `add` and `apply` leave the selection alone except
/// to drop deleted ids. [`undo`](Self::undo) and [`redo`](Self::redo) replace
/// the selection with the annotations the undone or redone step touched that
/// still exist, so the user sees what changed: undoing a move or delete selects
/// the moved or restored annotations, while undoing an add (or redoing a
/// delete) leaves nothing selected.
#[derive(Debug, Clone)]
pub struct Document {
    /// The image the document was opened with, which resizes resample.
    original: Arc<Image>,
    state: State,
    selection: BTreeSet<AnnotationId>,
    next_id: u64,
    history: History,
}

impl Document {
    /// A document with no annotations over `base`.
    #[must_use]
    pub fn new(base: Image) -> Self {
        let base = Arc::new(base);
        Self {
            original: Arc::clone(&base),
            state: State {
                base,
                annotations: Vec::new(),
                crop: None,
            },
            selection: BTreeSet::new(),
            next_id: 0,
            history: History::default(),
        }
    }

    /// The largest width or height [`resize`](Self::resize) makes the base
    /// image, in pixels.
    pub const MAX_SIDE: u32 = 16_384;

    /// The image being annotated, as resized.
    #[must_use]
    pub fn base(&self) -> &Image {
        &self.state.base
    }

    /// [`base`](Self::base), shared: it is a new one (not
    /// [`Arc::ptr_eq`]) after every resize, or undo or redo of one.
    #[must_use]
    pub fn shared_base(&self) -> &Arc<Image> {
        &self.state.base
    }

    /// The base image's extent in document coordinates: `(0, 0)` to
    /// `(width, height)`.
    #[must_use]
    pub fn bounds(&self) -> Rect {
        let size = Size::new(self.base().width() as f32, self.base().height() as f32);
        Rect::new(Point::ORIGIN, size)
    }

    /// The crop, if the document is cropped ([`Command::SetCrop`]).
    ///
    /// The crop is non-destructive: the base image and the annotations stay
    /// whole, in the same coordinates, and only flattening (for export)
    /// applies it, cutting the result down to the crop's pixels (its edges
    /// rounded to whole pixels and clipped to the image; a crop entirely
    /// outside the image is ignored). The crop tool keeps it on whole pixels
    /// within the image.
    #[must_use]
    pub const fn crop(&self) -> Option<Rect> {
        self.state.crop
    }

    /// The part of the document an export shows: the crop, or the whole
    /// image if uncropped.
    #[must_use]
    pub fn cropped_bounds(&self) -> Rect {
        self.crop().unwrap_or_else(|| self.bounds())
    }

    /// The base image's pixels an export keeps: the crop's
    /// [pixels](Rect::pixels) within the image. `None` if an export keeps
    /// them all: the document is uncropped, or cropped entirely outside the
    /// image.
    #[must_use]
    pub fn crop_pixels(&self) -> Option<PhysicalRect> {
        let image = PhysicalRect::new(0, 0, self.base().width(), self.base().height());
        self.crop()?.pixels()?.intersection(&image)
    }

    /// The size, in pixels, of the image an export produces: the
    /// [crop's](Self::crop_pixels), or the whole base image's.
    #[must_use]
    pub fn export_size(&self) -> PhysicalSize {
        self.crop_pixels()
            .map_or(self.base().size(), |pixels| pixels.size)
    }

    /// Resizes the document so that an export is `size` pixels, as one undo
    /// step. Returns false, changing nothing, if that is the size already,
    /// `size` or the image is empty, or the base image would be wider or
    /// taller than [`MAX_SIDE`](Self::MAX_SIDE).
    ///
    /// Everything scales by the export's factor on each axis: the base image
    /// is resampled ([`chartreuse_imaging::resize`], from the image the
    /// document was opened with), the annotations' geometry scales
    /// ([`Shape::scale`]), stroke widths and font sizes scale by the mean of
    /// the two factors (so a disproportionate resize moves and stretches
    /// shapes but keeps their strokes and text even), and the crop scales
    /// too, staying on whole pixels, exactly `size`. The selection is
    /// unchanged.
    pub fn resize(&mut self, size: PhysicalSize) -> bool {
        let current = self.export_size();
        if size == current || size.is_empty() || current.is_empty() {
            return false;
        }
        let x = size.width as f32 / current.width as f32;
        let y = size.height as f32 / current.height as f32;
        let side = |pixels: u32, factor: f32| ((pixels as f32 * factor).round() as u32).max(1);
        let mut base =
            PhysicalSize::new(side(self.base().width(), x), side(self.base().height(), y));
        let crop = self.crop_pixels().map(|crop| {
            // Whole pixels, and exactly `size`: the base grows to hold it if
            // rounding left it a pixel short.
            let origin = crop.origin;
            let (left, top) = (
                (origin.x as f32 * x).round() as i32,
                (origin.y as f32 * y).round() as i32,
            );
            let crop = PhysicalRect::new(left, top, size.width, size.height);
            base.width = base.width.max(crop.max_x() as u32);
            base.height = base.height.max(crop.max_y() as u32);
            Rect::from_pixels(crop)
        });
        if base.width > Self::MAX_SIDE || base.height > Self::MAX_SIDE {
            return false;
        }
        let Ok(image) = chartreuse_imaging::resize(&self.original, base) else {
            return false;
        };
        let length = (x * y).sqrt();
        let mut after = State {
            base: Arc::new(image),
            annotations: self.state.annotations.clone(),
            crop,
        };
        for annotation in &mut after.annotations {
            annotation.shape.scale(x, y);
            annotation.style.stroke_width *= length;
            annotation.style.font_size *= length;
        }
        self.commit(Edit::Replace {
            before: Box::new(self.state.clone()),
            after: Box::new(after),
        });
        true
    }

    /// Every annotation, bottom to top.
    #[must_use]
    pub fn annotations(&self) -> &[Annotation] {
        &self.state.annotations
    }

    #[must_use]
    pub fn get(&self, id: AnnotationId) -> Option<&Annotation> {
        self.index_of(id)
            .map(|index| &self.state.annotations[index])
    }

    /// The annotation's z-order position (0 = bottom).
    #[must_use]
    pub fn index_of(&self, id: AnnotationId) -> Option<usize> {
        self.state.annotations.iter().position(|a| a.id() == id)
    }

    /// The number step marker `id` shows: its 1-based rank, in creation
    /// order (by id), among the step markers in the document. `None` if `id`
    /// is not a step marker here. Derived, never stored, so deleting a
    /// marker renumbers the ones after it, undoing that restores the old
    /// numbers, and z-order changes leave the numbers alone.
    #[must_use]
    pub fn step_number(&self, id: AnnotationId) -> Option<usize> {
        let is_step = |a: &&Annotation| matches!(a.shape, Shape::Step(_));
        self.get(id).filter(is_step)?;
        Some(
            self.state
                .annotations
                .iter()
                .filter(is_step)
                .filter(|a| a.id() <= id)
                .count(),
        )
    }

    /// The number a step marker added now would show: one more than the
    /// number of step markers (new ids are the highest).
    #[must_use]
    pub fn next_step_number(&self) -> usize {
        self.state
            .annotations
            .iter()
            .filter(|a| matches!(a.shape, Shape::Step(_)))
            .count()
            + 1
    }

    /// Adds an annotation on top of the others and returns its new id; one
    /// undo step. The selection is unchanged; select the new annotation
    /// explicitly if wanted.
    pub fn add(&mut self, shape: Shape, style: Style) -> AnnotationId {
        let id = AnnotationId(self.next_id);
        self.next_id += 1;
        let edit = Edit::add(&self.state, Annotation::new(id, shape, style));
        self.commit(edit);
        id
    }

    /// Applies `command` as one undo step. Returns false, recording nothing, if
    /// it would change nothing (see [`Command`]).
    pub fn apply(&mut self, command: Command) -> bool {
        match Edit::plan(command, &self.state) {
            Some(edit) => {
                self.commit(edit);
                true
            }
            None => false,
        }
    }

    fn commit(&mut self, edit: Edit) {
        edit.apply(&mut self.state);
        self.history.record(edit);
        let annotations = &self.state.annotations;
        self.selection
            .retain(|id| annotations.iter().any(|a| a.id() == *id));
    }

    #[must_use]
    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    #[must_use]
    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// Reverts the latest recorded step. Returns false if there is none.
    pub fn undo(&mut self) -> bool {
        let touched = self.history.undo(&mut self.state);
        self.select_touched(touched)
    }

    /// Re-applies the latest undone step. Returns false if there is none.
    pub fn redo(&mut self) -> bool {
        let touched = self.history.redo(&mut self.state);
        self.select_touched(touched)
    }

    fn select_touched(&mut self, touched: Option<Vec<AnnotationId>>) -> bool {
        let Some(touched) = touched else {
            return false;
        };
        self.set_selection(touched);
        true
    }

    /// The topmost annotation that `point` hits, with `tolerance` in document
    /// units (see [`Shape::hit`]).
    ///
    /// A point outside the [cropped bounds](Self::cropped_bounds) hits
    /// nothing: the editor hides everything outside them (apart from while
    /// the crop tool edits the crop, which hit-tests nothing). So an
    /// annotation partly outside is found only by its visible part, give or
    /// take `tolerance` at the edge.
    #[must_use]
    pub fn annotation_at(&self, point: Point, tolerance: f32) -> Option<AnnotationId> {
        if !self.cropped_bounds().contains(point) {
            return None;
        }
        self.state
            .annotations
            .iter()
            .rev()
            .find(|a| a.hit(point, tolerance))
            .map(Annotation::id)
    }

    /// The selected ids.
    #[must_use]
    pub const fn selection(&self) -> &BTreeSet<AnnotationId> {
        &self.selection
    }

    /// The selected annotations, bottom to top.
    pub fn selected(&self) -> impl Iterator<Item = &Annotation> {
        self.state
            .annotations
            .iter()
            .filter(|a| self.selection.contains(&a.id()))
    }

    #[must_use]
    pub fn is_selected(&self, id: AnnotationId) -> bool {
        self.selection.contains(&id)
    }

    /// Adds `id` to the selection. Returns false (and does nothing) if no such
    /// annotation exists.
    pub fn select(&mut self, id: AnnotationId) -> bool {
        let exists = self.index_of(id).is_some();
        if exists {
            self.selection.insert(id);
        }
        exists
    }

    /// Removes `id` from the selection. Returns whether it was selected.
    pub fn deselect(&mut self, id: AnnotationId) -> bool {
        self.selection.remove(&id)
    }

    /// Replaces the selection, ignoring ids of annotations that don't exist.
    pub fn set_selection(&mut self, ids: impl IntoIterator<Item = AnnotationId>) {
        self.selection.clear();
        for id in ids {
            self.select(id);
        }
    }

    pub fn clear_selection(&mut self) {
        self.selection.clear();
    }

    /// Records the laid-out size of text annotation `id` for its current
    /// content and font size, replacing the estimate used for hit-testing and
    /// bounds. The canvas calls this after laying the text out; any later
    /// change to the content or font size clears it again. This is a cache,
    /// not an edit: it is not recorded for undo.
    ///
    /// Returns false (and stores nothing) if `id` is not a text annotation or
    /// `size` is not finite and non-negative.
    pub fn set_text_size(&mut self, id: AnnotationId, size: Size) -> bool {
        let valid = |v: f32| v.is_finite() && v >= 0.0;
        if !valid(size.width) || !valid(size.height) {
            return false;
        }
        let Some(index) = self.index_of(id) else {
            return false;
        };
        match &mut self.state.annotations[index].shape {
            Shape::Text(text) => {
                text.set_measured(Some(size));
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use chartreuse_core::color::Rgba8;
    use chartreuse_core::geometry::PhysicalSize;

    use super::super::annotation::{Arrow, Line, Rectangle, StepMarker, Text};
    use super::super::geometry::Vector;
    use super::super::history::Reorder;
    use super::super::style::StylePatch;
    use super::*;

    fn document() -> Document {
        Document::new(Image::filled(PhysicalSize::new(200, 100), Rgba8::WHITE))
    }

    fn line(ax: f32, ay: f32, bx: f32, by: f32) -> Shape {
        Shape::Line(Line {
            start: Point::new(ax, ay),
            end: Point::new(bx, by),
        })
    }

    fn rectangle(ax: f32, ay: f32, bx: f32, by: f32) -> Shape {
        Shape::Rectangle(Rectangle {
            rect: Rect::from_corners(Point::new(ax, ay), Point::new(bx, by)),
        })
    }

    #[test]
    fn bounds_cover_the_base_image() {
        let doc = document();
        assert_eq!(
            doc.bounds(),
            Rect::from_corners(Point::ORIGIN, Point::new(200.0, 100.0))
        );
    }

    #[test]
    fn resizing_resamples_the_image_and_scales_everything_on_it() {
        let original = Image::from_fn(PhysicalSize::new(200, 100), |x, y| {
            Rgba8::rgb(x as u8, y as u8, 0)
        });
        let mut doc = Document::new(original.clone());
        let a = doc.add(line(20.0, 10.0, 100.0, 50.0), Style::default());
        let ends = |doc: &Document| match &doc.get(a).unwrap().shape {
            Shape::Line(line) => (line.start, line.end),
            other => panic!("not a line: {other:?}"),
        };

        // Uncropped, the base becomes exactly the size.
        assert!(doc.resize(PhysicalSize::new(100, 25)));
        let small = chartreuse_imaging::resize(&original, PhysicalSize::new(100, 25)).unwrap();
        assert!(doc.base() == &small);
        assert_eq!(ends(&doc), (Point::new(10.0, 2.5), Point::new(50.0, 12.5)));
        let width = doc.get(a).unwrap().style.stroke_width;
        assert_eq!(
            width,
            Style::default().stroke_width * (0.5_f32 * 0.25).sqrt()
        );

        // Resizing again resamples the original, not the smaller image.
        assert!(doc.resize(PhysicalSize::new(200, 100)));
        assert!(doc.base() == &original);
        assert_eq!(
            ends(&doc),
            (Point::new(20.0, 10.0), Point::new(100.0, 50.0))
        );

        // Cropped, the crop scales too, and the export is exactly the size.
        let crop = Rect::from_corners(Point::new(33.0, 7.0), Point::new(134.0, 90.0));
        doc.apply(Command::SetCrop(Some(crop)));
        assert!(doc.resize(PhysicalSize::new(67, 41)));
        assert_eq!(doc.export_size(), PhysicalSize::new(67, 41));

        // One undo step.
        assert!(doc.undo());
        assert_eq!(doc.export_size(), PhysicalSize::new(101, 83));
        assert!(doc.base() == &original);

        // Nothing to do, or a base too big: no resize.
        assert!(!doc.resize(PhysicalSize::new(101, 83)));
        assert!(!doc.resize(PhysicalSize::new(Document::MAX_SIDE, 83)));
        assert_eq!(doc.export_size(), PhysicalSize::new(101, 83));
    }

    #[test]
    fn step_numbers_follow_creation_order_through_delete_undo_and_reorder() {
        let mut doc = document();
        let step = |x: f32| {
            Shape::Step(StepMarker {
                center: Point::new(x, 50.0),
            })
        };
        assert_eq!(doc.next_step_number(), 1);
        let a = doc.add(step(10.0), Style::default());
        let other = doc.add(line(0.0, 0.0, 5.0, 5.0), Style::default());
        let b = doc.add(step(20.0), Style::default());
        let c = doc.add(step(30.0), Style::default());
        let numbers = |doc: &Document| [a, b, c].map(|id| doc.step_number(id));
        assert_eq!(numbers(&doc), [Some(1), Some(2), Some(3)]);
        assert_eq!(doc.step_number(other), None, "not a step marker");
        assert_eq!(doc.next_step_number(), 4);

        // Deleting the second renumbers the third; undo and redo follow.
        doc.apply(Command::Delete { ids: vec![b] });
        assert_eq!(numbers(&doc), [Some(1), None, Some(2)]);
        assert_eq!(doc.next_step_number(), 3);
        doc.undo();
        assert_eq!(numbers(&doc), [Some(1), Some(2), Some(3)]);
        doc.redo();
        assert_eq!(numbers(&doc), [Some(1), None, Some(2)]);
        doc.undo();

        // Bringing the first to the front does not renumber.
        doc.apply(Command::Reorder {
            ids: vec![a],
            step: Reorder::ToFront,
        });
        assert_eq!(numbers(&doc), [Some(1), Some(2), Some(3)]);
    }

    #[test]
    fn added_annotations_stack_on_top_with_increasing_ids() {
        let mut doc = document();
        let a = doc.add(line(0.0, 0.0, 10.0, 10.0), Style::default());
        let b = doc.add(rectangle(0.0, 0.0, 10.0, 10.0), Style::default());
        assert!(a < b);
        assert_eq!(doc.index_of(a), Some(0));
        assert_eq!(doc.index_of(b), Some(1));
        assert_eq!(doc.get(b).map(Annotation::id), Some(b));
        assert!(doc.selection().is_empty());
    }

    #[test]
    fn annotation_at_returns_the_topmost_hit() {
        let mut doc = document();
        let bottom = doc.add(line(0.0, 50.0, 200.0, 50.0), Style::default());
        let top = doc.add(line(100.0, 0.0, 100.0, 100.0), Style::default());
        assert_eq!(doc.annotation_at(Point::new(100.0, 50.0), 0.0), Some(top));
        assert_eq!(doc.annotation_at(Point::new(20.0, 50.0), 0.0), Some(bottom));
        assert_eq!(doc.annotation_at(Point::new(20.0, 20.0), 0.0), None);
    }

    #[test]
    fn annotation_at_sees_through_rectangle_interiors() {
        let mut doc = document();
        let inner = doc.add(line(40.0, 50.0, 60.0, 50.0), Style::default());
        doc.add(rectangle(10.0, 10.0, 190.0, 90.0), Style::default());
        assert_eq!(doc.annotation_at(Point::new(50.0, 50.0), 0.0), Some(inner));
    }

    #[test]
    fn selection_holds_only_existing_annotations() {
        let mut doc = document();
        let a = doc.add(line(0.0, 0.0, 1.0, 1.0), Style::default());
        let b = doc.add(line(0.0, 0.0, 1.0, 1.0), Style::default());
        let ghost = AnnotationId(99);
        assert!(!doc.select(ghost));
        doc.set_selection([b, ghost, a]);
        assert_eq!(doc.selection().iter().copied().collect::<Vec<_>>(), [a, b]);
        let ids: Vec<_> = doc.selected().map(Annotation::id).collect();
        assert_eq!(ids, [a, b]);
        assert!(doc.deselect(a));
        assert!(!doc.deselect(a));
        assert!(doc.is_selected(b));
        doc.clear_selection();
        assert!(doc.selection().is_empty());
    }

    #[test]
    fn text_size_is_stored_only_for_text_and_valid_sizes() {
        let mut doc = document();
        let text = doc.add(
            Shape::Text(Text::new(Point::ORIGIN, "hi")),
            Style::default(),
        );
        let line = doc.add(line(0.0, 0.0, 1.0, 1.0), Style::default());
        assert!(!doc.set_text_size(line, Size::new(10.0, 10.0)));
        assert!(!doc.set_text_size(text, Size::new(f32::NAN, 10.0)));
        assert!(!doc.set_text_size(text, Size::new(-1.0, 10.0)));
        assert!(doc.set_text_size(text, Size::new(80.0, 30.0)));
        assert_eq!(
            doc.get(text).unwrap().bounds(),
            Rect::from_corners(Point::ORIGIN, Point::new(80.0, 30.0))
        );
        assert_eq!(doc.annotation_at(Point::new(79.0, 29.0), 0.0), Some(text));
    }

    fn text(content: &str) -> Shape {
        Shape::Text(Text::new(Point::new(10.0, 10.0), content))
    }

    fn ids(doc: &Document) -> Vec<AnnotationId> {
        doc.annotations().iter().map(Annotation::id).collect()
    }

    fn selection(doc: &Document) -> Vec<AnnotationId> {
        doc.selection().iter().copied().collect()
    }

    fn measured(doc: &Document, id: AnnotationId) -> Option<Size> {
        match &doc.get(id).unwrap().shape {
            Shape::Text(text) => text.measured(),
            other => panic!("not text: {other:?}"),
        }
    }

    #[test]
    fn add_is_undoable_and_redo_restores_the_same_id() {
        let mut doc = document();
        assert!(!doc.can_undo() && !doc.can_redo());
        let a = doc.add(line(0.0, 0.0, 10.0, 10.0), Style::default());
        let added = doc.annotations().to_vec();
        assert!(doc.can_undo());

        assert!(doc.undo());
        assert!(doc.annotations().is_empty());
        assert!(!doc.can_undo() && doc.can_redo());
        assert!(!doc.undo());

        assert!(doc.redo());
        assert_eq!(ids(&doc), [a]);
        assert_eq!(doc.annotations(), added.as_slice());
        assert!(!doc.redo());
    }

    #[test]
    fn ids_are_never_reused_after_undo() {
        let mut doc = document();
        let a = doc.add(line(0.0, 0.0, 10.0, 10.0), Style::default());
        doc.undo();
        let b = doc.add(line(0.0, 0.0, 10.0, 10.0), Style::default());
        assert!(b > a);
        assert!(doc.get(a).is_none());
    }

    #[test]
    fn a_new_step_clears_the_redo_stack() {
        let mut doc = document();
        let a = doc.add(line(0.0, 0.0, 10.0, 10.0), Style::default());
        assert!(doc.apply(Command::Translate {
            ids: vec![a],
            delta: Vector::new(5.0, 0.0),
        }));
        doc.undo();
        assert!(doc.can_redo());
        assert!(doc.apply(Command::Translate {
            ids: vec![a],
            delta: Vector::new(0.0, 5.0),
        }));
        assert!(!doc.can_redo() && !doc.redo());

        // Adding clears it too.
        doc.undo();
        assert!(doc.can_redo());
        doc.add(line(0.0, 0.0, 1.0, 1.0), Style::default());
        assert!(!doc.can_redo());

        // A no-op does not.
        doc.undo();
        assert!(!doc.apply(Command::Delete { ids: vec![] }));
        assert!(doc.can_redo());
    }

    #[test]
    fn no_op_commands_are_not_recorded() {
        let mut doc = document();
        let a = doc.add(line(0.0, 0.0, 10.0, 10.0), Style::default());
        let t = doc.add(text("hi"), Style::default());
        let ghost = AnnotationId(99);
        let delta = Vector::new(5.0, 5.0);
        let noops = [
            Command::Translate {
                ids: vec![a],
                delta: Vector::ZERO,
            },
            Command::Translate { ids: vec![], delta },
            Command::Translate {
                ids: vec![ghost],
                delta,
            },
            Command::Translate {
                ids: vec![a],
                delta: Vector::new(f32::NAN, 0.0),
            },
            Command::Translate {
                ids: vec![a],
                delta: Vector::new(f32::INFINITY, 0.0),
            },
            Command::Restyle {
                ids: vec![a, t],
                patch: StylePatch {
                    color: Some(Style::DEFAULT_COLOR),
                    ..StylePatch::default()
                },
            },
            Command::Restyle {
                ids: vec![a],
                patch: StylePatch::default(),
            },
            Command::Reshape {
                id: a,
                shape: line(0.0, 0.0, 10.0, 10.0),
            },
            Command::Reshape {
                id: ghost,
                shape: line(0.0, 0.0, 1.0, 1.0),
            },
            Command::EditText {
                id: t,
                content: "hi".into(),
            },
            Command::EditText {
                id: a,
                content: "not text".into(),
            },
            Command::Delete { ids: vec![ghost] },
            Command::Reorder {
                ids: vec![t],
                step: Reorder::ToFront,
            },
            Command::Reorder {
                ids: vec![t],
                step: Reorder::Forward,
            },
            Command::Reorder {
                ids: vec![a],
                step: Reorder::ToBack,
            },
            Command::Reorder {
                ids: vec![a],
                step: Reorder::Backward,
            },
            Command::Reorder {
                ids: vec![a, t],
                step: Reorder::Forward,
            },
            Command::Reorder {
                ids: vec![ghost],
                step: Reorder::ToBack,
            },
        ];
        let before = doc.annotations().to_vec();
        for command in noops {
            assert!(!doc.apply(command.clone()), "{command:?} was applied");
        }
        assert_eq!(doc.annotations(), before.as_slice());
        // The two adds are still the only steps.
        assert!(doc.undo() && doc.undo());
        assert!(!doc.undo());
    }

    #[test]
    fn crop_is_an_undoable_step_that_leaves_the_annotations_alone() {
        let mut doc = document();
        let a = doc.add(line(0.0, 0.0, 10.0, 10.0), Style::default());
        doc.set_selection([a]);
        let crop = Rect::from_corners(Point::new(10.0, 20.0), Point::new(110.0, 80.0));
        assert_eq!(doc.cropped_bounds(), doc.bounds(), "uncropped");

        assert!(doc.apply(Command::SetCrop(Some(crop))));
        assert_eq!(doc.crop(), Some(crop));
        assert_eq!(doc.cropped_bounds(), crop);
        assert!(doc.is_selected(a), "applying keeps the selection");
        assert!(
            !doc.apply(Command::SetCrop(Some(crop))),
            "unchanged: not recorded"
        );

        // Undo restores the crop before, selecting nothing, since it touched
        // no annotation; redo brings the crop back.
        assert!(doc.undo());
        assert_eq!(doc.crop(), None);
        assert!(doc.selection().is_empty());
        assert!(doc.redo());
        assert_eq!(doc.crop(), Some(crop));

        assert!(doc.apply(Command::SetCrop(None)));
        assert_eq!(doc.crop(), None);
        assert!(doc.undo() && doc.undo());
        assert_eq!(doc.crop(), None);
        assert_eq!(doc.annotations().len(), 1, "the add is still there");
    }

    #[test]
    fn translate_moves_each_annotation_once_and_undo_restores_exactly() {
        let mut doc = document();
        let a = doc.add(line(0.1, 0.2, 10.3, 10.7), Style::default());
        doc.add(rectangle(1.0, 1.0, 5.0, 5.0), Style::default());
        let c = doc.add(text("x"), Style::default());
        let before = doc.annotations().to_vec();
        let delta = Vector::new(0.1, 0.7);

        assert!(doc.apply(Command::Translate {
            ids: vec![a, c, a],
            delta,
        }));
        let moved = |index: usize| {
            let mut shape = before[index].shape.clone();
            shape.translate(delta);
            shape
        };
        assert_eq!(doc.annotations()[0].shape, moved(0));
        assert_eq!(doc.annotations()[1], before[1]);
        assert_eq!(doc.annotations()[2].shape, moved(2));

        assert!(doc.undo());
        assert_eq!(doc.annotations(), before.as_slice());
    }

    #[test]
    fn restyle_undo_restores_each_previous_style() {
        let mut doc = document();
        let thin_blue = Style {
            color: Rgba8::rgb(0, 0, 255),
            stroke_width: 2.0,
            ..Style::default()
        };
        let thick = Style {
            stroke_width: 8.0,
            ..Style::default()
        };
        let a = doc.add(line(0.0, 0.0, 1.0, 1.0), thin_blue);
        let b = doc.add(line(0.0, 0.0, 1.0, 1.0), thick);
        let green = Rgba8::rgb(0, 255, 0);
        assert!(doc.apply(Command::Restyle {
            ids: vec![a, b],
            patch: StylePatch {
                color: Some(green),
                ..StylePatch::default()
            },
        }));
        assert_eq!(doc.get(a).unwrap().style.color, green);
        assert_eq!(doc.get(a).unwrap().style.stroke_width, 2.0);
        assert_eq!(doc.get(b).unwrap().style.color, green);
        assert_eq!(doc.get(b).unwrap().style.stroke_width, 8.0);

        doc.undo();
        assert_eq!(doc.get(a).unwrap().style, thin_blue);
        assert_eq!(doc.get(b).unwrap().style, thick);
    }

    #[test]
    fn restyle_changes_each_field_only_where_the_kind_draws_with_it() {
        let mut doc = document();
        let a = doc.add(line(0.0, 0.0, 1.0, 1.0), Style::default());
        let t = doc.add(text("hi"), Style::default());
        let red = Rgba8::rgb(255, 0, 0);
        assert!(doc.apply(Command::Restyle {
            ids: vec![a, t],
            patch: StylePatch {
                color: Some(red),
                font_size: Some(40.0),
                ..StylePatch::default()
            },
        }));
        let (line_style, text_style) = (doc.get(a).unwrap().style, doc.get(t).unwrap().style);
        assert_eq!((line_style.color, line_style.font_size), (red, 24.0));
        assert_eq!((text_style.color, text_style.font_size), (red, 40.0));

        // A stroke width means nothing to text: nothing to record.
        assert!(!doc.apply(Command::Restyle {
            ids: vec![t],
            patch: StylePatch {
                stroke_width: Some(10.0),
                ..StylePatch::default()
            },
        }));
    }

    #[test]
    fn delete_restores_positions_ids_and_selection_on_undo() {
        let mut doc = document();
        let all: Vec<_> = [0.0, 10.0, 20.0, 30.0, 40.0]
            .into_iter()
            .map(|y| doc.add(line(0.0, y, 100.0, y), Style::default()))
            .collect();
        let before = doc.annotations().to_vec();
        doc.set_selection([all[1], all[3], all[4]]);

        assert!(doc.apply(Command::Delete {
            ids: vec![all[3], all[1], AnnotationId(99)],
        }));
        assert_eq!(ids(&doc), [all[0], all[2], all[4]]);
        assert_eq!(selection(&doc), [all[4]]);

        assert!(doc.undo());
        assert_eq!(doc.annotations(), before.as_slice());
        assert_eq!(selection(&doc), [all[1], all[3]]);

        assert!(doc.redo());
        assert_eq!(ids(&doc), [all[0], all[2], all[4]]);
        assert!(doc.selection().is_empty());
    }

    #[test]
    fn reorder_changes_the_topmost_hit_and_is_undoable() {
        let mut doc = document();
        let a = doc.add(line(0.0, 50.0, 200.0, 50.0), Style::default());
        let b = doc.add(line(100.0, 0.0, 100.0, 100.0), Style::default());
        let c = doc.add(line(0.0, 0.0, 200.0, 100.0), Style::default());
        let cross = Point::new(100.0, 50.0);
        assert_eq!(doc.annotation_at(cross, 0.0), Some(c));

        assert!(doc.apply(Command::Reorder {
            ids: vec![a],
            step: Reorder::ToFront,
        }));
        assert_eq!(ids(&doc), [b, c, a]);
        assert_eq!(doc.annotation_at(cross, 0.0), Some(a));

        assert!(doc.apply(Command::Reorder {
            ids: vec![a],
            step: Reorder::Backward,
        }));
        assert_eq!(ids(&doc), [b, a, c]);

        assert!(doc.undo());
        assert_eq!(ids(&doc), [b, c, a]);
        assert_eq!(selection(&doc), [a]);
        assert!(doc.undo());
        assert_eq!(ids(&doc), [a, b, c]);
        assert!(doc.redo() && doc.redo());
        assert_eq!(ids(&doc), [b, a, c]);
    }

    #[test]
    fn undo_and_redo_select_what_the_step_touched() {
        let mut doc = document();
        let a = doc.add(line(0.0, 0.0, 1.0, 1.0), Style::default());
        let b = doc.add(line(0.0, 0.0, 1.0, 1.0), Style::default());
        doc.set_selection([a, b]);
        assert!(doc.apply(Command::Translate {
            ids: vec![a],
            delta: Vector::new(1.0, 0.0),
        }));
        // Applying leaves the selection alone.
        assert_eq!(selection(&doc), [a, b]);
        doc.clear_selection();

        doc.undo();
        assert_eq!(selection(&doc), [a]);
        doc.set_selection([b]);
        doc.redo();
        assert_eq!(selection(&doc), [a]);

        // Undoing the move, then the add of `b`: `b` is gone, so nothing is
        // selected.
        doc.undo();
        doc.set_selection([b]);
        doc.undo();
        assert!(doc.selection().is_empty());
        // Redoing the add selects the restored annotation.
        doc.redo();
        assert_eq!(selection(&doc), [b]);
    }

    #[test]
    fn text_edits_are_undoable_and_invalidate_the_measurement() {
        let mut doc = document();
        let t = doc.add(text("hi"), Style::default());
        let hi_size = Size::new(30.0, 29.0);
        doc.set_text_size(t, hi_size);

        assert!(doc.apply(Command::EditText {
            id: t,
            content: "hello".into(),
        }));
        let Shape::Text(edited) = &doc.get(t).unwrap().shape else {
            unreachable!()
        };
        assert_eq!(edited.content, "hello");
        assert_eq!(edited.measured(), None);
        doc.set_text_size(t, Size::new(60.0, 29.0));

        // Undo brings back the old text with the measurement taken for it.
        doc.undo();
        let Shape::Text(restored) = &doc.get(t).unwrap().shape else {
            unreachable!()
        };
        assert_eq!(restored.content, "hi");
        assert_eq!(restored.measured(), Some(hi_size));
        // Redo brings back the new text, to be measured again.
        doc.redo();
        assert_eq!(measured(&doc, t), None);
    }

    #[test]
    fn font_size_changes_invalidate_the_text_measurement_but_color_does_not() {
        let mut doc = document();
        let t = doc.add(text("hi"), Style::default());
        let size = Size::new(30.0, 29.0);
        doc.set_text_size(t, size);
        assert!(doc.apply(Command::Restyle {
            ids: vec![t],
            patch: StylePatch {
                color: Some(Rgba8::BLACK),
                ..StylePatch::default()
            },
        }));
        assert_eq!(measured(&doc, t), Some(size));
        assert!(doc.apply(Command::Restyle {
            ids: vec![t],
            patch: StylePatch {
                font_size: Some(40.0),
                ..StylePatch::default()
            },
        }));
        assert_eq!(measured(&doc, t), None);
    }

    #[test]
    fn reshape_replaces_geometry_keeping_the_measurement_of_unchanged_text() {
        let mut doc = document();
        let a = doc.add(line(0.0, 0.0, 10.0, 10.0), Style::default());
        assert!(doc.apply(Command::Reshape {
            id: a,
            shape: line(0.0, 0.0, 50.0, 10.0),
        }));
        assert_eq!(doc.get(a).unwrap().shape, line(0.0, 0.0, 50.0, 10.0));
        doc.undo();
        assert_eq!(doc.get(a).unwrap().shape, line(0.0, 0.0, 10.0, 10.0));

        let t = doc.add(text("hi"), Style::default());
        let size = Size::new(30.0, 29.0);
        doc.set_text_size(t, size);
        let moved = Shape::Text(Text::new(Point::new(50.0, 50.0), "hi"));
        assert!(doc.apply(Command::Reshape {
            id: t,
            shape: moved
        }));
        assert_eq!(measured(&doc, t), Some(size));
        let rewritten = Shape::Text(Text::new(Point::new(50.0, 50.0), "bye"));
        assert!(doc.apply(Command::Reshape {
            id: t,
            shape: rewritten,
        }));
        assert_eq!(measured(&doc, t), None);
    }

    /// A small deterministic generator (an LCG) for the replay test.
    struct Rng(u64);

    impl Rng {
        fn below(&mut self, n: usize) -> usize {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            usize::try_from(self.0 >> 33).unwrap() % n
        }

        fn coord(&mut self) -> f32 {
            f32::from(u16::try_from(self.below(2000)).unwrap()) / 10.0
        }

        fn some_of(&mut self, ids: &[AnnotationId]) -> Vec<AnnotationId> {
            ids.iter().copied().filter(|_| self.below(3) == 0).collect()
        }
    }

    #[test]
    fn undo_and_redo_replay_a_long_random_session_exactly() {
        let mut rng = Rng(7);
        let mut doc = document();
        let state = |doc: &Document| (doc.annotations().to_vec(), doc.crop());
        let mut states = vec![state(&doc)];
        for _ in 0..500 {
            let existing = ids(&doc);
            let changed = match rng.below(9) {
                0 | 1 => {
                    let (ax, ay, bx, by) = (rng.coord(), rng.coord(), rng.coord(), rng.coord());
                    let shape = match rng.below(4) {
                        0 => line(ax, ay, bx, by),
                        1 => Shape::Arrow(Arrow {
                            start: Point::new(ax, ay),
                            end: Point::new(bx, by),
                        }),
                        2 => rectangle(ax, ay, bx, by),
                        _ => Shape::Text(Text::new(Point::new(ax, ay), "note")),
                    };
                    doc.add(shape, Style::default());
                    true
                }
                2 => doc.apply(Command::Translate {
                    ids: rng.some_of(&existing),
                    delta: Vector::new(rng.coord() - 100.0, rng.coord() - 100.0),
                }),
                3 => doc.apply(Command::Restyle {
                    ids: rng.some_of(&existing),
                    patch: StylePatch {
                        stroke_width: Some(rng.coord()),
                        font_size: (rng.below(2) == 0).then(|| rng.coord()),
                        ..StylePatch::default()
                    },
                }),
                4 => doc.apply(Command::Delete {
                    ids: rng.some_of(&existing),
                }),
                5 if !existing.is_empty() => doc.apply(Command::EditText {
                    id: existing[rng.below(existing.len())],
                    content: format!("edit {}", rng.below(100)),
                }),
                6 if !existing.is_empty() => doc.apply(Command::Reshape {
                    id: existing[rng.below(existing.len())],
                    shape: line(rng.coord(), rng.coord(), rng.coord(), rng.coord()),
                }),
                7 => doc.apply(Command::SetCrop((rng.below(3) > 0).then(|| {
                    Rect::from_corners(
                        Point::new(rng.coord(), rng.coord()),
                        Point::new(rng.coord(), rng.coord()),
                    )
                }))),
                _ => doc.apply(Command::Reorder {
                    ids: rng.some_of(&existing),
                    step: [
                        Reorder::Forward,
                        Reorder::Backward,
                        Reorder::ToFront,
                        Reorder::ToBack,
                    ][rng.below(4)],
                }),
            };
            if changed {
                states.push(state(&doc));
            } else {
                assert_eq!(&state(&doc), states.last().unwrap());
            }
        }
        assert!(states.len() > 300, "too few steps: {}", states.len());

        for expected in states.iter().rev().skip(1) {
            assert!(doc.undo());
            assert_eq!(&state(&doc), expected);
        }
        assert!(!doc.undo());
        for expected in states.iter().skip(1) {
            assert!(doc.redo());
            assert_eq!(&state(&doc), expected);
        }
        assert!(!doc.redo());
    }
}

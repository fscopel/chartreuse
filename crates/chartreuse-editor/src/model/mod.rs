//! The document model: base image, ordered annotations, selection, and
//! command-based undo/redo. Pure logic. Owned by track 1G.
//!
//! # Coordinate space
//!
//! Everything in the model is in **base-image pixel coordinates**, as `f32`:
//! the origin is the top-left corner of the base image, `x` grows right, `y`
//! grows down, and one unit is one pixel of the base image (not a logical
//! point, and independent of the editor's zoom). Coordinates are continuous and
//! may fall outside the image; flattening clips to the image.
//!
//! The canvas maps between widget and document space with its own zoom/pan
//! transform, and must scale screen-space quantities (such as a hit-test
//! tolerance of a few logical points) into document units before calling the
//! model.
//!
//! # Overview
//!
//! - [`Document`] owns the base image, the [`Annotation`]s in z-order (index 0
//!   at the bottom), the selection, and the undo history.
//! - An [`Annotation`] is a stable [`AnnotationId`], a [`Shape`] (one variant
//!   per kind: [`Line`], [`Arrow`], [`Rectangle`], [`Ellipse`], a pen's or
//!   [`highlighter`]'s [`Polyline`], [`StepMarker`], [`BlurRegion`],
//!   [`Text`]) and a [`Style`]. Step markers' numbers are derived:
//!   [`Document::step_number`].
//! - Edits go through [`Document::add`] and [`Document::apply`] with a
//!   [`Command`]; each is one undo step, and no-ops are not recorded.
//! - [`Document::crop`] is the document's non-destructive crop, set (and
//!   undone) like any edit, with [`Command::SetCrop`].
//! - [`Document::annotation_at`] finds the topmost annotation under a point
//!   within the cropped bounds; [`Shape::hit`] documents the hit area of
//!   each kind.
//!
//! The module docs at the top of `model/annotation.rs` and `model/history.rs`
//! describe how new annotation kinds and document-level settings slot in.

mod annotation;
mod document;
mod geometry;
mod history;
mod style;

pub use annotation::{
    highlighter, Annotation, AnnotationId, Arrow, ArrowHead, BlurRegion, Ellipse, Line, Polyline,
    Rectangle, Shape, StepMarker, Text,
};
pub use document::Document;
pub use geometry::{
    distance_to_ellipse, distance_to_polyline, distance_to_segment, distance_to_triangle, Point,
    Rect, Size, Vector,
};
pub use history::{Command, Reorder};
pub use style::{BlurMode, Style, StyleFields, StylePatch};

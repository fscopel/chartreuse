//! Commands and the undo/redo history.
//!
//! A [`Command`] describes a user edit in terms of intent ("move these by
//! that"). Applying it computes an [`Edit`]: the exact before/after state of
//! everything it touches. Edits, not commands, go on the history, so undo and
//! redo restore bit-identical state (no float drift from moving back by
//! `-delta`) and each kind of edit has one inverse.
//!
//! # Extending
//!
//! Edits are planned against and applied to a [`State`], which holds
//! everything undoable: the annotations and the document-level crop. A new
//! document-level setting is a [`State`] field, a [`Command`] that sets it,
//! and an [`Edit`] holding its value before and after, as the crop's
//! `SetCrop` does: `apply` sets the field and returns no ids (it touches no
//! annotations, so undoing or redoing it leaves nothing selected),
//! `inverted` swaps before and after, and `is_empty` compares them.
//!
//! New annotation kinds need nothing here.

use std::collections::{BTreeSet, HashMap};

use super::annotation::{Annotation, AnnotationId, Shape};
use super::geometry::{Rect, Vector};
use super::style::StylePatch;

/// A recordable edit to a [`Document`](super::Document)'s annotations, applied
/// with [`Document::apply`](super::Document::apply). (Adding is
/// [`Document::add`](super::Document::add), which also returns the new id.)
///
/// Each command is one undo step, so tools commit a whole gesture (a finished
/// drag, a finished text edit) as one command and preview the in-progress state
/// themselves.
///
/// Ids of annotations that don't exist are ignored, as are repeated ids. A
/// command that would change nothing is not recorded.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Moves annotations by `delta` (ignored unless finite).
    Translate {
        ids: Vec<AnnotationId>,
        delta: Vector,
    },
    /// Applies `patch` to each annotation's style.
    Restyle {
        ids: Vec<AnnotationId>,
        patch: StylePatch,
    },
    /// Replaces one annotation's shape, for resize handles and endpoint drags.
    Reshape { id: AnnotationId, shape: Shape },
    /// Replaces a text annotation's content (ignored for other kinds).
    EditText { id: AnnotationId, content: String },
    /// Removes annotations.
    Delete { ids: Vec<AnnotationId> },
    /// Moves annotations in the z-order; see [`Reorder`].
    Reorder {
        ids: Vec<AnnotationId>,
        step: Reorder,
    },
    /// Crops the document to a rectangle, or (`None`) uncrops it. The crop
    /// is non-destructive: the base image and the annotations are kept
    /// whole, and only flattening applies it (see
    /// [`Document::crop`](super::Document::crop)).
    SetCrop(Option<Rect>),
}

/// A z-order change for a set of annotations (the targets). Targets keep their
/// order relative to each other, and so do all the other annotations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Reorder {
    /// Up one step: each run of adjacent targets swaps places with the
    /// annotation just above it. Targets with nothing but targets above them
    /// stay put.
    Forward,
    /// Down one step, the mirror image of `Forward`.
    Backward,
    /// Above every other annotation.
    ToFront,
    /// Below every other annotation.
    ToBack,
}

/// The part of a document that edits change and undo restores.
#[derive(Debug, Clone, Default)]
pub(super) struct State {
    /// Bottom to top.
    pub(super) annotations: Vec<Annotation>,
    /// `None`: uncropped.
    pub(super) crop: Option<Rect>,
}

/// An annotation at its z-order index.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Placed {
    index: usize,
    annotation: Annotation,
}

/// One annotation's state before and after an edit.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Change {
    index: usize,
    before: Annotation,
    after: Annotation,
}

/// A recorded, exactly invertible change to a [`State`].
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Edit {
    /// Inserts annotations; sorted by ascending index, each index being the
    /// annotation's position once all are inserted.
    Insert(Vec<Placed>),
    /// Removes annotations; the same layout as `Insert` (positions before
    /// removal).
    Remove(Vec<Placed>),
    /// Replaces annotations in place.
    Modify(Vec<Change>),
    /// Permutes the list from the `before` id order to `after`. `moved` are the
    /// targets, which undo and redo select.
    Reorder {
        before: Vec<AnnotationId>,
        after: Vec<AnnotationId>,
        moved: Vec<AnnotationId>,
    },
    /// Changes the crop.
    SetCrop {
        before: Option<Rect>,
        after: Option<Rect>,
    },
}

impl Edit {
    /// Adding `annotation` on top of `state`'s annotations.
    pub(super) fn add(state: &State, annotation: Annotation) -> Self {
        Self::Insert(vec![Placed {
            index: state.annotations.len(),
            annotation,
        }])
    }

    /// The edit `command` makes to `state`, or `None` if it would change
    /// nothing.
    pub(super) fn plan(command: Command, state: &State) -> Option<Self> {
        let annotations = &state.annotations;
        let edit = match command {
            Command::Translate { ids, delta } => {
                if !delta.is_finite() {
                    return None;
                }
                modify(annotations, &ids, |a| a.shape.translate(delta))
            }
            Command::Restyle { ids, patch } => modify(annotations, &ids, |a| {
                let style = a.style.patched(&patch);
                if style.font_size != a.style.font_size {
                    clear_measurement(&mut a.shape);
                }
                a.style = style;
            }),
            Command::Reshape { id, mut shape } => modify(annotations, &[id], |a| {
                keep_measurement_if_same_text(&mut shape, &a.shape);
                a.shape = shape.clone();
            }),
            Command::EditText { id, content } => modify(annotations, &[id], |a| {
                if let Shape::Text(text) = &mut a.shape
                    && text.content != content
                {
                    text.content.clone_from(&content);
                    text.set_measured(None);
                }
            }),
            Command::Delete { ids } => {
                let ids: BTreeSet<_> = ids.into_iter().collect();
                Self::Remove(
                    annotations
                        .iter()
                        .enumerate()
                        .filter(|(_, a)| ids.contains(&a.id()))
                        .map(|(index, a)| Placed {
                            index,
                            annotation: a.clone(),
                        })
                        .collect(),
                )
            }
            Command::Reorder { ids, step } => {
                let targets: BTreeSet<_> = ids.into_iter().collect();
                let before: Vec<_> = annotations.iter().map(Annotation::id).collect();
                let after = reordered(&before, &targets, step);
                if after == before {
                    return None;
                }
                let moved = before
                    .iter()
                    .copied()
                    .filter(|id| targets.contains(id))
                    .collect();
                Self::Reorder {
                    before,
                    after,
                    moved,
                }
            }
            Command::SetCrop(crop) => Self::SetCrop {
                before: state.crop,
                after: crop,
            },
        };
        (!edit.is_empty()).then_some(edit)
    }

    fn is_empty(&self) -> bool {
        match self {
            Self::Insert(placed) | Self::Remove(placed) => placed.is_empty(),
            Self::Modify(changes) => changes.is_empty(),
            Self::Reorder { before, after, .. } => before == after,
            Self::SetCrop { before, after } => before == after,
        }
    }

    /// The edit that undoes this one.
    fn inverted(self) -> Self {
        match self {
            Self::Insert(placed) => Self::Remove(placed),
            Self::Remove(placed) => Self::Insert(placed),
            Self::Modify(changes) => Self::Modify(
                changes
                    .into_iter()
                    .map(|c| Change {
                        index: c.index,
                        before: c.after,
                        after: c.before,
                    })
                    .collect(),
            ),
            Self::Reorder {
                before,
                after,
                moved,
            } => Self::Reorder {
                before: after,
                after: before,
                moved,
            },
            Self::SetCrop { before, after } => Self::SetCrop {
                before: after,
                after: before,
            },
        }
    }

    /// Applies the edit, which must have been planned against (or inverted
    /// from an edit applied to) exactly this state. Returns the ids of the
    /// annotations it touched.
    pub(super) fn apply(&self, state: &mut State) -> Vec<AnnotationId> {
        let annotations = &mut state.annotations;
        match self {
            Self::Insert(placed) => {
                for p in placed {
                    annotations.insert(p.index, p.annotation.clone());
                }
                placed.iter().map(|p| p.annotation.id()).collect()
            }
            Self::Remove(placed) => {
                for p in placed.iter().rev() {
                    let removed = annotations.remove(p.index);
                    debug_assert_eq!(removed.id(), p.annotation.id());
                }
                placed.iter().map(|p| p.annotation.id()).collect()
            }
            Self::Modify(changes) => {
                for c in changes {
                    // Only the id: the text measurement cache may have been
                    // filled in since `before` was taken.
                    debug_assert_eq!(annotations[c.index].id(), c.before.id());
                    annotations[c.index] = c.after.clone();
                }
                changes.iter().map(|c| c.after.id()).collect()
            }
            Self::Reorder { after, moved, .. } => {
                let rank: HashMap<_, _> =
                    after.iter().enumerate().map(|(i, id)| (*id, i)).collect();
                debug_assert_eq!(rank.len(), annotations.len());
                annotations.sort_by_key(|a| rank.get(&a.id()).copied().unwrap_or(usize::MAX));
                moved.clone()
            }
            Self::SetCrop { after, .. } => {
                state.crop = *after;
                Vec::new()
            }
        }
    }
}

/// A `Modify` of the annotations in `ids`, each changed by `change`; the ones
/// `change` leaves equal are left out.
fn modify(
    annotations: &[Annotation],
    ids: &[AnnotationId],
    mut change: impl FnMut(&mut Annotation),
) -> Edit {
    let ids: BTreeSet<_> = ids.iter().copied().collect();
    Edit::Modify(
        annotations
            .iter()
            .enumerate()
            .filter(|(_, a)| ids.contains(&a.id()))
            .filter_map(|(index, before)| {
                let mut after = before.clone();
                change(&mut after);
                (after != *before).then(|| Change {
                    index,
                    before: before.clone(),
                    after,
                })
            })
            .collect(),
    )
}

fn clear_measurement(shape: &mut Shape) {
    if let Shape::Text(text) = shape {
        text.set_measured(None);
    }
}

/// Keeps `old`'s text measurement on `new` when both are text with the same
/// content (so moving text by reshaping it keeps its measured size); clears it
/// otherwise, so a measurement never outlives its layout.
fn keep_measurement_if_same_text(new: &mut Shape, old: &Shape) {
    if let Shape::Text(new) = new {
        let measured = match old {
            Shape::Text(old) if old.content == new.content => old.measured(),
            _ => None,
        };
        new.set_measured(measured);
    }
}

/// `order` with `targets` moved by `step`.
fn reordered(
    order: &[AnnotationId],
    targets: &BTreeSet<AnnotationId>,
    step: Reorder,
) -> Vec<AnnotationId> {
    let is_target = |id: &AnnotationId| targets.contains(id);
    let mut result = order.to_vec();
    match step {
        Reorder::ToFront => result.sort_by_key(is_target),
        Reorder::ToBack => result.sort_by_key(|id| !is_target(id)),
        // Sweeping from the top down, a target below a non-target swaps with
        // it; the swapped non-target then meets the next target below, so a
        // whole run of targets moves up past it together.
        Reorder::Forward => {
            for i in (1..result.len()).rev() {
                if is_target(&result[i - 1]) && !is_target(&result[i]) {
                    result.swap(i - 1, i);
                }
            }
        }
        Reorder::Backward => {
            for i in 1..result.len() {
                if is_target(&result[i]) && !is_target(&result[i - 1]) {
                    result.swap(i - 1, i);
                }
            }
        }
    }
    result
}

/// Undo and redo stacks. Unbounded: edits are small (annotation snapshots, not
/// pixels).
#[derive(Debug, Clone, Default)]
pub(super) struct History {
    undo: Vec<Edit>,
    redo: Vec<Edit>,
}

impl History {
    /// Records an already-applied edit. Clears the redo stack.
    pub(super) fn record(&mut self, edit: Edit) {
        self.undo.push(edit);
        self.redo.clear();
    }

    pub(super) fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub(super) fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Reverts the latest edit, returning the ids it touched, or `None` if
    /// there is nothing to undo.
    pub(super) fn undo(&mut self, state: &mut State) -> Option<Vec<AnnotationId>> {
        Self::transfer(&mut self.undo, &mut self.redo, state)
    }

    /// Re-applies the latest undone edit, returning the ids it touched, or
    /// `None` if there is nothing to redo.
    pub(super) fn redo(&mut self, state: &mut State) -> Option<Vec<AnnotationId>> {
        Self::transfer(&mut self.redo, &mut self.undo, state)
    }

    /// Pops an edit, applies its inverse, and pushes that inverse on `to`
    /// (undoing an undo is redoing, and vice versa).
    fn transfer(
        from: &mut Vec<Edit>,
        to: &mut Vec<Edit>,
        state: &mut State,
    ) -> Option<Vec<AnnotationId>> {
        let inverse = from.pop()?.inverted();
        let touched = inverse.apply(state);
        to.push(inverse);
        Some(touched)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: u64) -> Vec<AnnotationId> {
        (0..n).map(AnnotationId).collect()
    }

    /// Checks `reordered` against its specification for every subset of
    /// targets in a stack of `n`.
    fn check_reorder(n: u64, step: Reorder) {
        let order = ids(n);
        for mask in 0..(1_u32 << n) {
            let targets: BTreeSet<_> = order
                .iter()
                .copied()
                .filter(|id| mask & (1 << id.0) != 0)
                .collect();
            let result = reordered(&order, &targets, step);
            let context = format!("{step:?} of {targets:?} gave {result:?}");

            // A permutation that keeps the relative order within targets and
            // within the rest.
            let is_target = |id: &AnnotationId| targets.contains(id);
            let (moved, rest): (Vec<AnnotationId>, Vec<_>) =
                result.iter().partition(|id| is_target(id));
            let (moved_before, rest_before): (Vec<AnnotationId>, Vec<_>) =
                order.iter().partition(|id| is_target(id));
            assert_eq!(moved, moved_before, "{context}");
            assert_eq!(rest, rest_before, "{context}");

            let position = |id: AnnotationId| result.iter().position(|x| *x == id).unwrap();
            let k = targets.len();
            for (old, &id) in order.iter().enumerate() {
                if !is_target(&id) {
                    continue;
                }
                let new = position(id);
                let above = order[old + 1..].iter().any(|x| !is_target(x));
                let below = order[..old].iter().any(|x| !is_target(x));
                match step {
                    Reorder::ToFront => assert!(new >= order.len() - k, "{context}"),
                    Reorder::ToBack => assert!(new < k, "{context}"),
                    Reorder::Forward => {
                        assert_eq!(new, if above { old + 1 } else { old }, "{context}");
                    }
                    Reorder::Backward => {
                        assert_eq!(new, if below { old - 1 } else { old }, "{context}");
                    }
                }
            }
        }
    }

    #[test]
    fn reorder_meets_its_specification_for_every_target_set() {
        for step in [
            Reorder::Forward,
            Reorder::Backward,
            Reorder::ToFront,
            Reorder::ToBack,
        ] {
            for n in 0..=6 {
                check_reorder(n, step);
            }
        }
    }

    #[test]
    fn forward_moves_a_run_past_one_annotation() {
        let order = ids(4);
        let targets = [AnnotationId(0), AnnotationId(1)].into_iter().collect();
        let [a, b, c, d] = [0, 1, 2, 3].map(AnnotationId);
        assert_eq!(reordered(&order, &targets, Reorder::Forward), [c, a, b, d]);
        assert_eq!(reordered(&order, &targets, Reorder::Backward), order);
        let targets = [a, d].into_iter().collect();
        assert_eq!(reordered(&order, &targets, Reorder::Forward), [b, a, c, d]);
        assert_eq!(reordered(&order, &targets, Reorder::Backward), [a, b, d, c]);
    }
}

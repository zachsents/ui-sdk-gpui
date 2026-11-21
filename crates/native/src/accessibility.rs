//! What assistive technology reads of a field: its text as runs, one or
//! more per visual row, with each character's place, the words, and the
//! selection; whether it's read-only, disabled or required; and the
//! actions it takes, setting the value, replacing the selection and
//! selecting.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::accesskit::{
    self, ActionData, NodeId, Rect, TextDirection, TextPosition, TextSelection,
};
use gpui::{A11ySubtreeBuilder, Bounds, Context, Pixels, Point, Role, Window, point, px};
use ui_sdk_kernel::ChangeSource;
use unicode_segmentation::UnicodeSegmentation;

use crate::display::Display;
use crate::editing::Change;
use crate::events::{InputType, SelectionDirection};
use crate::history::Group;
use crate::layout::TextLayout;
use crate::text_field::TextField;

// A run's characters are indexed in bytes, so a long row splits into runs.
const RUN_CHARS: usize = 200;

/// One text run as the platform reads it.
pub(crate) struct A11yRun {
    row: usize,
    text: String,
    lengths: Vec<u8>,
    // The value offset each character starts at, and where the last ends.
    starts: Vec<usize>,
    positions: Vec<f32>,
    widths: Vec<f32>,
    word_starts: Vec<u8>,
    bounds: Rect,
}

/// What the field's runs and selection report, written each frame the tree
/// is built, and read back by the actions.
#[derive(Default)]
pub(crate) struct A11ySnapshot {
    runs: Vec<A11yRun>,
    anchor: Option<(usize, usize)>,
    focus: Option<(usize, usize)>,
    ids: Vec<NodeId>,
}

/// The facts about the field a snapshot needs besides its layout.
pub(crate) struct SnapshotInput<'a> {
    pub layout: &'a TextLayout,
    pub display: &'a Display,
    pub origin: Point<Pixels>,
    pub scale: f32,
    pub anchor: usize,
    pub head: usize,
    pub upstream: bool,
}

impl A11ySnapshot {
    /// Records the text as it was laid out this frame.
    pub fn update(&mut self, input: SnapshotInput) {
        let SnapshotInput {
            layout,
            display,
            origin,
            scale,
            anchor,
            head,
            upstream,
        } = input;
        let text = display.text.as_ref();
        let last_row = layout.rows.len() - 1;
        self.runs.clear();
        for (index, row) in layout.rows.iter().enumerate() {
            let mut row_text = text.get(row.start..row.end).unwrap_or_default().to_string();
            let line_break = !row.wraps && index < last_row;
            if line_break {
                row_text.push('\n');
            }
            let word_starts: Vec<usize> = row_text
                .split_word_bound_indices()
                .map(|(start, _)| start)
                .collect();
            let graphemes: Vec<(usize, &str)> = row_text.grapheme_indices(true).collect();
            let top = origin.y + layout.line_height * index as f32;
            let mut chunk_start = 0;
            loop {
                let chunk = &graphemes[chunk_start..(chunk_start + RUN_CHARS).min(graphemes.len())];
                let mut run = A11yRun {
                    row: index,
                    text: String::new(),
                    lengths: Vec::with_capacity(chunk.len()),
                    starts: Vec::with_capacity(chunk.len() + 1),
                    positions: Vec::with_capacity(chunk.len()),
                    widths: Vec::with_capacity(chunk.len()),
                    word_starts: Vec::new(),
                    bounds: Rect::default(),
                };
                let first_x = chunk
                    .first()
                    .map(|(start, _)| layout.x_for(index, row.start + start))
                    .unwrap_or_else(|| layout.x_for(index, row.start));
                let mut end_x = first_x;
                // Past a line break is the next paragraph's start.
                let mut end = display.to_value(row.start);
                for (number, (start, grapheme)) in chunk.iter().enumerate() {
                    let shown = (row.start + start).min(row.end);
                    let x = layout.x_for(index, shown);
                    let width = if *grapheme == "\n" {
                        end = display.to_value(row.end) + 1;
                        px(0.)
                    } else {
                        let next = (shown + grapheme.len()).min(row.end);
                        end = display.to_value(next);
                        layout.x_for(index, next) - x
                    };
                    run.text.push_str(grapheme);
                    run.lengths.push(grapheme.len().min(u8::MAX as usize) as u8);
                    run.starts.push(display.to_value(shown));
                    run.positions.push(((x - first_x) * scale).as_f32());
                    run.widths.push((width * scale).as_f32());
                    if word_starts.contains(start) {
                        run.word_starts.push(number as u8);
                    }
                    end_x = x + width;
                }
                run.starts.push(end);
                run.bounds = rect(
                    Bounds::from_corners(
                        point(origin.x + first_x, top),
                        point(origin.x + end_x, top + layout.line_height),
                    ),
                    scale,
                );
                self.runs.push(run);
                chunk_start += RUN_CHARS;
                if chunk_start >= graphemes.len() {
                    break;
                }
            }
        }
        self.anchor = self.position(layout, display, anchor, anchor == head && upstream);
        self.focus = self.position(layout, display, head, upstream);
    }

    // The run and character a value offset is at.
    fn position(
        &self,
        layout: &TextLayout,
        display: &Display,
        offset: usize,
        upstream: bool,
    ) -> Option<(usize, usize)> {
        let row = layout.row_for(display.to_display(offset), upstream);
        let mut found = None;
        for (index, run) in self.runs.iter().enumerate() {
            if run.row != row {
                continue;
            }
            let chars = run.lengths.len();
            let ends_line = run.text.ends_with('\n');
            let last = if ends_line { chars - 1 } else { chars };
            let character = run.starts[..=last].partition_point(|start| *start < offset);
            found = Some((index, character.min(last)));
            if offset < run.starts[last] {
                break;
            }
        }
        found
    }

    // The value offset of a position the platform names.
    fn offset_of(&self, position: &TextPosition) -> Option<usize> {
        let index = self.ids.iter().position(|id| *id == position.node)?;
        let run = &self.runs[index];
        Some(run.starts[position.character_index.min(run.starts.len() - 1)])
    }
}

fn rect(bounds: Bounds<Pixels>, scale: f32) -> Rect {
    let scaled = |pixels: Pixels| (pixels * scale).to_f64();
    Rect {
        x0: scaled(bounds.origin.x),
        y0: scaled(bounds.origin.y),
        x1: scaled(bounds.origin.x + bounds.size.width),
        y1: scaled(bounds.origin.y + bounds.size.height),
    }
}

/// Adds the runs under the field's node and its selection, and makes a
/// textbox the kind of text input accesskit has for it, like a password's,
/// unless a component gave the part a role of its own.
pub(crate) fn build(
    snapshot: &Rc<RefCell<A11ySnapshot>>,
    native_role: Role,
    builder: &mut A11ySubtreeBuilder,
) {
    let mut snapshot = snapshot.borrow_mut();
    let ids: Vec<NodeId> = (0..snapshot.runs.len())
        .map(|index| builder.synthetic_node_id(("text-run", index)))
        .collect();
    for (index, run) in snapshot.runs.iter().enumerate() {
        let mut node = accesskit::Node::new(Role::TextRun);
        node.set_value(run.text.clone());
        node.set_character_lengths(run.lengths.clone());
        node.set_character_positions(run.positions.clone());
        node.set_character_widths(run.widths.clone());
        node.set_word_starts(run.word_starts.clone());
        node.set_text_direction(TextDirection::LeftToRight);
        node.set_bounds(run.bounds);
        if index > 0 && snapshot.runs[index - 1].row == run.row {
            node.set_previous_on_line(ids[index - 1]);
        }
        if snapshot.runs.get(index + 1).is_some_and(|next| next.row == run.row) {
            node.set_next_on_line(ids[index + 1]);
        }
        builder.push_child(ids[index], node);
    }
    let parent = builder.parent_node();
    if matches!(parent.role(), Role::TextInput | Role::SearchInput) {
        parent.set_role(native_role);
    }
    if let (Some(anchor), Some(focus)) = (snapshot.anchor, snapshot.focus) {
        parent.set_text_selection(TextSelection {
            anchor: TextPosition {
                node: ids[anchor.0],
                character_index: anchor.1,
            },
            focus: TextPosition {
                node: ids[focus.0],
                character_index: focus.1,
            },
        });
    }
    snapshot.ids = ids;
}

/// The text an action carries.
pub(crate) fn value_of(data: Option<&ActionData>) -> Option<String> {
    match data {
        Some(ActionData::Value(value)) => Some(value.to_string()),
        _ => None,
    }
}

impl TextField {
    // Assistive technology setting the whole value.
    pub(crate) fn a11y_set_value(
        &mut self,
        value: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let len = self.value.len();
        self.apply(
            Change {
                input_type: InputType::InsertReplacementText,
                range: 0..len,
                text: value,
                source: ChangeSource::Virtual,
                user: true,
                group: Group::Single,
                selection: None,
            },
            window,
            cx,
        );
    }

    // Assistive technology typing over the selection.
    pub(crate) fn a11y_replace(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_composition(window, cx);
        self.apply(
            Change {
                input_type: InputType::InsertText,
                range: self.start..self.end,
                text,
                source: ChangeSource::Virtual,
                user: true,
                group: Group::Single,
                selection: None,
            },
            window,
            cx,
        );
    }

    // Assistive technology moving the selection.
    pub(crate) fn a11y_select(
        &mut self,
        data: Option<&ActionData>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ActionData::SetTextSelection(selection)) = data else {
            return;
        };
        let (anchor, focus) = {
            let snapshot = self.a11y.borrow();
            match (
                snapshot.offset_of(&selection.anchor),
                snapshot.offset_of(&selection.focus),
            ) {
                (Some(anchor), Some(focus)) => (anchor, focus),
                _ => return,
            }
        };
        let (start, end, direction) = if focus < anchor {
            (focus, anchor, SelectionDirection::Backward)
        } else if focus > anchor {
            (anchor, focus, SelectionDirection::Forward)
        } else {
            (focus, focus, SelectionDirection::None)
        };
        self.set_selection_range(start, end, direction, window, cx);
    }
}

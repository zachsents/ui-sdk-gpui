//! The element inside a field's part that lays its text out and paints it:
//! the text or the placeholder, the selection, the caret and the marked
//! text, scrolled to keep the caret in view. It sizes like an `<input>`,
//! `cols` characters wide, or its style's `ch`, and a line tall, or like a
//! `<textarea>`, `rows` lines tall or as tall as its text.

use std::ops::Range;
use std::rc::Rc;

use gpui::{
    App, AvailableSpace, Bounds, ContentMask, DispatchPhase, Element, ElementId, ElementInputHandler,
    Entity, GlobalElementId, InspectorElementId, LayoutId, MouseButton,
    MouseMoveEvent, Pixels, Point, SharedString, Size, Style, TextRun, TextStyle, Window, fill,
    point, px, size,
};

use crate::accessibility::SnapshotInput;
use crate::layout::TextLayout;
use crate::style::ResolvedStyle;
use crate::text_field::{Geometry, TextField};

pub(crate) struct TextFieldElement {
    field: Entity<TextField>,
}

impl TextFieldElement {
    pub fn new(field: Entity<TextField>) -> Self {
        Self { field }
    }
}

/// What a frame paints.
pub(crate) struct Frame {
    layout: Rc<TextLayout>,
    placeholder: Option<TextLayout>,
    scroll: Point<Pixels>,
    selection: Vec<Bounds<Pixels>>,
    caret: Option<Point<Pixels>>,
    colors: ResolvedStyle,
    line_height: Pixels,
    dragging: bool,
    blur: bool,
}

impl Element for TextFieldElement {
    type RequestLayoutState = ();
    type PrepaintState = Option<Frame>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let field = self.field.read(cx);
        let multi_line = field.multi_line;
        let rows = field.rows;
        let chars = field.style.width_in_ch(field.cols);
        let auto_resize = multi_line && field.auto_resize;
        // A field sized to its text sizes to its placeholder while it's empty.
        let text = if field.value.is_empty() {
            field.placeholder.clone().unwrap_or_default()
        } else {
            field.display().text
        };
        let text_style = window.text_style();
        let font_size = text_style.font_size.to_pixels(window.rem_size());
        let line_height = window.line_height();
        let run = text_style.to_run(text.len());
        let zero = text_style.to_run(1);

        let mut style = Style::default();
        style.flex_shrink = 1.;
        style.min_size.width = px(0.).into();
        style.min_size.height = px(0.).into();
        if multi_line {
            style.flex_grow = 1.;
        }
        let layout_id = window.request_measured_layout(
            style,
            move |known: Size<Option<Pixels>>, _available: Size<AvailableSpace>, window, _cx| {
                let width = known.width.unwrap_or_else(|| {
                    // The width of that many `0`s in its font and features,
                    // as CSS's `ch` measures.
                    let digit = window
                        .text_system()
                        .layout_line("0", font_size, std::slice::from_ref(&zero), None)
                        .width;
                    digit * chars
                });
                let height = known.height.unwrap_or_else(|| {
                    if !multi_line {
                        line_height
                    } else if auto_resize {
                        let rows = window
                            .text_system()
                            .shape_text(text.clone(), font_size, std::slice::from_ref(&run), Some(width), None)
                            .map(|lines| {
                                lines
                                    .iter()
                                    .map(|line| line.wrap_boundaries().len() + 1)
                                    .sum::<usize>()
                            })
                            .unwrap_or(1);
                        line_height * rows.max(1) as f32
                    } else {
                        line_height * rows as f32
                    }
                });
                size(width, height)
            },
        );
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let text_style = window.text_style();
        let font_size = text_style.font_size.to_pixels(window.rem_size());
        let line_height = window.line_height();
        let field = self.field.read(cx);
        // The view the text draws in, which the field notifies as it
        // changes, even inside a cached view.
        field.instance.record_view(window);
        let align = field.style.align(text_style.text_align, ui_sdk_platform::is_rtl(cx));
        let focused = field.focus_handle.is_focused(window);
        let colors = field.style.resolve(text_style.color);
        let display = field.display();
        let single_line = !field.multi_line;
        let marked = field
            .composition
            .as_ref()
            .map(|composition| {
                display.to_display(composition.range.start)..display.to_display(composition.range.end)
            });
        let shown_selection = display.to_display(field.start)..display.to_display(field.end);
        let selected = (focused && !shown_selection.is_empty() && colors.selected_text != colors.text)
            .then(|| shown_selection.clone());
        let runs = text_runs(&text_style, display.text.len(), &colors, marked.clone(), selected);
        let wrap_width = (!single_line).then_some(bounds.size.width);
        let layout = TextLayout::shape(
            &display.text,
            &runs,
            font_size,
            wrap_width,
            line_height,
            bounds.size.width,
            align,
            single_line,
            window,
        );
        let placeholder = field
            .placeholder
            .clone()
            .filter(|placeholder| {
                !placeholder.is_empty() && field.value.is_empty() && field.composition.is_none()
            })
            .map(|placeholder| {
                let placeholder: SharedString = if single_line {
                    placeholder.replace('\n', " ").into()
                } else {
                    placeholder
                };
                let mut run = text_style.to_run(placeholder.len());
                run.color = colors.placeholder;
                TextLayout::shape(
                    &placeholder,
                    &[run],
                    font_size,
                    wrap_width,
                    line_height,
                    bounds.size.width,
                    align,
                    single_line,
                    window,
                )
            });

        let head = display.to_display(field.head());
        let caret = layout.caret(head, field.upstream);
        let caret_width = colors.caret_width;
        let mut scroll = field.scroll;
        // Only a focused field follows its caret.
        if field.autoscroll && focused {
            if single_line {
                if caret.x < scroll.x {
                    scroll.x = caret.x;
                } else if caret.x + caret_width > scroll.x + bounds.size.width {
                    scroll.x = caret.x + caret_width - bounds.size.width;
                }
            } else if caret.y < scroll.y {
                scroll.y = caret.y;
            } else if caret.y + line_height > scroll.y + bounds.size.height {
                scroll.y = caret.y + line_height - bounds.size.height;
            }
        }
        if single_line {
            let max = (layout.content_width() + caret_width - bounds.size.width).max(px(0.));
            scroll.x = scroll.x.clamp(px(0.), max);
            scroll.y = px(0.);
        } else {
            let max = (layout.content_height() - bounds.size.height).max(px(0.));
            scroll.y = scroll.y.clamp(px(0.), max);
            scroll.x = px(0.);
        }

        let selection = if focused && !shown_selection.is_empty() {
            layout.range_rects(shown_selection)
        } else {
            Vec::new()
        };
        let caret_shown = focused
            && field.caret_visible
            && field.is_editable()
            && field.start == field.end;
        let caret = caret_shown.then(|| {
            // Kept inside the text area, where alignment would put it on the edge.
            let x = (caret.x - scroll.x).clamp(px(0.), (bounds.size.width - caret_width).max(px(0.)));
            point(x + scroll.x, caret.y)
        });
        let dragging = field.is_dragging();
        let blur = focused && field.disabled;
        let anchor = field.anchor();
        let head_offset = field.head();
        let upstream = field.upstream;
        let snapshot = field.a11y.clone();

        let layout = Rc::new(layout);
        let display = Rc::new(display);
        if window.is_a11y_active() {
            snapshot.borrow_mut().update(SnapshotInput {
                layout: &layout,
                display: &display,
                origin: bounds.origin - scroll,
                scale: window.scale_factor(),
                anchor,
                head: head_offset,
                upstream,
            });
        }
        let geometry = Geometry {
            layout: layout.clone(),
            display,
            bounds,
            scroll,
        };
        self.field.update(cx, |field, _| {
            field.geometry = Some(geometry);
            field.scroll = scroll;
            field.autoscroll = false;
            field.sync_mirror();
        });

        Some(Frame {
            layout,
            placeholder,
            scroll,
            selection,
            caret,
            colors,
            line_height,
            dragging,
            blur,
        })
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        frame: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(frame) = frame.take() else {
            return;
        };
        let focus_handle = self.field.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.field.clone()),
            cx,
        );
        let origin = bounds.origin - frame.scroll;
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for rect in &frame.selection {
                window.paint_quad(fill(
                    Bounds::new(rect.origin + origin, rect.size),
                    frame.colors.selection,
                ));
            }
            match &frame.placeholder {
                Some(placeholder) => placeholder.paint(origin, window, cx),
                None => frame.layout.paint(origin, window, cx),
            }
            if let Some(caret) = frame.caret {
                window.paint_quad(fill(
                    Bounds::new(caret + origin, size(frame.colors.caret_width, frame.line_height)),
                    frame.colors.caret,
                ));
            }
        });
        // A drag goes on past the field's edges, and ends wherever the button rises.
        if frame.dragging {
            let field = self.field.downgrade();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                if phase != DispatchPhase::Capture {
                    return;
                }
                field
                    .update(cx, |field, cx| {
                        if event.pressed_button == Some(MouseButton::Left) {
                            field.drag_to(event.position, window, cx);
                        } else {
                            field.end_drag();
                        }
                    })
                    .ok();
            });
        }
        // A field disabled while focused lets focus go, as the browser does.
        if frame.blur {
            window.defer(cx, move |window, cx| {
                if focus_handle.is_focused(window) {
                    window.blur(cx);
                }
            });
        }
    }
}

// The runs of the shown text: its color, the selected text's, and the
// underline under what an input method is composing.
fn text_runs(
    style: &TextStyle,
    len: usize,
    colors: &ResolvedStyle,
    marked: Option<Range<usize>>,
    selected: Option<Range<usize>>,
) -> Vec<TextRun> {
    if len == 0 {
        return Vec::new();
    }
    let mut cuts = vec![0, len];
    for range in marked.iter().chain(selected.iter()) {
        cuts.push(range.start.min(len));
        cuts.push(range.end.min(len));
    }
    cuts.sort_unstable();
    cuts.dedup();
    let within = |range: &Option<Range<usize>>, start: usize| {
        range
            .as_ref()
            .is_some_and(|range| range.start <= start && start < range.end)
    };
    let base = style.to_run(0);
    cuts.windows(2)
        .map(|pair| {
            let start = pair[0];
            TextRun {
                len: pair[1] - start,
                color: if within(&selected, start) {
                    colors.selected_text
                } else {
                    colors.text
                },
                underline: within(&marked, start).then_some(colors.composition_underline),
                ..base.clone()
            }
        })
        .collect()
}

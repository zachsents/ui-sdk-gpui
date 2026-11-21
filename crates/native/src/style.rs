//! How a text field draws what the app's styles can't reach: the colors and
//! sizes of its text, caret, selection and placeholder, an alignment that
//! follows the reading direction, and a width in `ch`. Everything defaults
//! to the text color the field inherits, so it has no colors of its own. The
//! font, its size, its features, the line height and the alignment come
//! from the text style on the field's part, like `.text_size()`,
//! `.font_features()` for tabular numbers (`tnum`), or `.text_center()`.

use gpui::{Hsla, Pixels, Role, TextAlign, UnderlineStyle, px};

/// Where a field's text sits across it, `text-align`. `Start` and `End`
/// follow the app's reading direction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TextFieldAlign {
    #[default]
    Start,
    Center,
    End,
}

/// The colors and sizes a text field paints with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextFieldStyle {
    /// The text. Defaults to the inherited text color.
    pub text_color: Option<Hsla>,
    /// The placeholder. Defaults to the text color at half opacity.
    pub placeholder_color: Option<Hsla>,
    /// The caret. Defaults to the text color.
    pub caret_color: Option<Hsla>,
    pub caret_width: Pixels,
    /// Behind selected text. Defaults to the text color at a fifth of its
    /// opacity.
    pub selection_color: Option<Hsla>,
    /// Selected text. Defaults to the text color.
    pub selected_text_color: Option<Hsla>,
    /// Under text an input method is composing. With no color, the text's.
    pub composition_underline: UnderlineStyle,
    /// Where the text, the placeholder and the caret sit across the field.
    /// Without one, the part's text style aligns them, like `.text_center()`.
    pub text_align: Option<TextFieldAlign>,
    /// How wide the field is when nothing sets its width, in `ch`: widths of
    /// the `0` in its own font, as in CSS's `width: 2.6ch`. Without one,
    /// it's `cols` of them.
    pub width_ch: Option<f32>,
}

impl Default for TextFieldStyle {
    fn default() -> Self {
        Self {
            text_color: None,
            placeholder_color: None,
            caret_color: None,
            caret_width: px(1.),
            selection_color: None,
            selected_text_color: None,
            composition_underline: UnderlineStyle {
                thickness: px(1.),
                color: None,
                wavy: false,
            },
            text_align: None,
            width_ch: None,
        }
    }
}

impl TextFieldStyle {
    pub fn text_color(mut self, color: impl Into<Hsla>) -> Self {
        self.text_color = Some(color.into());
        self
    }

    pub fn placeholder_color(mut self, color: impl Into<Hsla>) -> Self {
        self.placeholder_color = Some(color.into());
        self
    }

    pub fn caret_color(mut self, color: impl Into<Hsla>) -> Self {
        self.caret_color = Some(color.into());
        self
    }

    pub fn caret_width(mut self, width: Pixels) -> Self {
        self.caret_width = width;
        self
    }

    pub fn selection_color(mut self, color: impl Into<Hsla>) -> Self {
        self.selection_color = Some(color.into());
        self
    }

    pub fn selected_text_color(mut self, color: impl Into<Hsla>) -> Self {
        self.selected_text_color = Some(color.into());
        self
    }

    pub fn composition_underline(mut self, underline: UnderlineStyle) -> Self {
        self.composition_underline = underline;
        self
    }

    pub fn text_align(mut self, align: TextFieldAlign) -> Self {
        self.text_align = Some(align);
        self
    }

    pub fn width_ch(mut self, width: f32) -> Self {
        self.width_ch = Some(width);
        self
    }

    /// How a frame aligns the text: the style's alignment in the app's
    /// direction, else the part's.
    pub(crate) fn align(&self, inherited: TextAlign, rtl: bool) -> TextAlign {
        match (self.text_align, rtl) {
            (None, _) => inherited,
            (Some(TextFieldAlign::Center), _) => TextAlign::Center,
            (Some(TextFieldAlign::Start), false) | (Some(TextFieldAlign::End), true) => TextAlign::Left,
            (Some(TextFieldAlign::Start), true) | (Some(TextFieldAlign::End), false) => TextAlign::Right,
        }
    }

    /// How many `ch` wide the field is when nothing sets its width: the
    /// style's width, else `cols`. A width CSS would refuse counts as none.
    pub(crate) fn width_in_ch(&self, cols: usize) -> f32 {
        self.width_ch
            .filter(|width| width.is_finite() && *width >= 0.)
            .unwrap_or(cols as f32)
    }

    pub(crate) fn resolve(&self, inherited: Hsla) -> ResolvedStyle {
        let text = self.text_color.unwrap_or(inherited);
        ResolvedStyle {
            text,
            placeholder: self.placeholder_color.unwrap_or(text.opacity(0.5)),
            caret: self.caret_color.unwrap_or(text),
            caret_width: self.caret_width,
            selection: self.selection_color.unwrap_or(text.opacity(0.2)),
            selected_text: self.selected_text_color.unwrap_or(text),
            composition_underline: UnderlineStyle {
                color: Some(self.composition_underline.color.unwrap_or(text)),
                ..self.composition_underline
            },
        }
    }
}

/// The colors a frame paints with, once the inherited text color is known.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ResolvedStyle {
    pub text: Hsla,
    pub placeholder: Hsla,
    pub caret: Hsla,
    pub caret_width: Pixels,
    pub selection: Hsla,
    pub selected_text: Hsla,
    pub composition_underline: UnderlineStyle,
}

/// What a single-line field holds, `<input type>`: its role for assistive
/// technology, and for `Password` and `Number`, how it behaves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TextFieldKind {
    #[default]
    Text,
    Search,
    Email,
    Tel,
    Url,
    /// Takes the characters a number is written with, and steps with the
    /// arrow keys, `type="number"`.
    Number,
    /// Shows a dot for each character, and never copies or cuts.
    Password,
}

impl TextFieldKind {
    /// The browser's name for it, like `"password"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Search => "search",
            Self::Email => "email",
            Self::Tel => "tel",
            Self::Url => "url",
            Self::Number => "number",
            Self::Password => "password",
        }
    }

    pub(crate) fn role(self, multi_line: bool) -> Role {
        if multi_line {
            return Role::MultilineTextInput;
        }
        match self {
            Self::Text => Role::TextInput,
            Self::Search => Role::SearchInput,
            Self::Email => Role::EmailInput,
            Self::Tel => Role::PhoneNumberInput,
            Self::Url => Role::UrlInput,
            Self::Number => Role::NumberInput,
            Self::Password => Role::PasswordInput,
        }
    }
}

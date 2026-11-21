//! The keys a text field answers, as each platform's text fields do: Option
//! and Command on a Mac, with its Control-key Emacs moves, and Control
//! elsewhere. A key no command claims is left to whatever listens above, and
//! a key that types is left to the platform's text input.

use gpui::{Keystroke, OperatingSystem};

/// Where a move takes the caret.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Movement {
    Left,
    Right,
    WordLeft,
    /// To the end of the next word: a Mac, and Linux.
    WordRight,
    /// To the start of the next word: Windows.
    NextWordStart,
    /// The start of the visual line.
    LineStart,
    LineEnd,
    ParagraphStart,
    ParagraphEnd,
    /// The start of this paragraph, or of the one before when already there.
    ParagraphBackward,
    /// The end of this paragraph, or of the one after when already there.
    ParagraphForward,
    Up,
    Down,
    PageUp,
    PageDown,
    Start,
    End,
}

/// What a deletion removes, when nothing is selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Deletion {
    CharBackward,
    CharForward,
    WordBackward,
    /// To the end of the next word.
    WordForward,
    /// To the start of the next word: Windows.
    WordForwardToStart,
    LineBackward,
    LineForward,
    /// To the end of the paragraph, and the Mac's kill buffer.
    ParagraphForward,
}

/// What a key does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Command {
    Move(Movement, bool),
    Delete(Deletion),
    LineBreak,
    /// Control-O on a Mac: a line break after the caret.
    OpenLine,
    /// Enter in a single-line field.
    Commit,
    SelectAll,
    Copy,
    Cut,
    Paste,
    Undo,
    Redo,
    Yank,
    Transpose,
    /// A number field's arrows.
    Step(i8),
    CharacterPalette,
}

/// The command a keystroke runs in a field, if any.
pub(crate) fn command_for(
    keystroke: &Keystroke,
    os: OperatingSystem,
    multi_line: bool,
    number: bool,
) -> Option<Command> {
    if matches!(os, OperatingSystem::Mac | OperatingSystem::Ios) {
        mac(keystroke, multi_line, number)
    } else {
        other(keystroke, os, multi_line, number)
    }
}

fn mac(keystroke: &Keystroke, multi_line: bool, number: bool) -> Option<Command> {
    use Command::*;
    use Movement::*;
    let modifiers = keystroke.modifiers;
    let (ctrl, alt, shift, cmd) = (
        modifiers.control,
        modifiers.alt,
        modifiers.shift,
        modifiers.platform,
    );
    let key = keystroke.key.as_str();
    if ctrl && !cmd && !alt {
        return match key {
            "a" => Some(Move(ParagraphStart, shift)),
            "e" => Some(Move(ParagraphEnd, shift)),
            "b" => Some(Move(Left, shift)),
            "f" => Some(Move(Right, shift)),
            "p" => Some(Move(Up, shift)),
            "n" => Some(Move(Down, shift)),
            "d" if !shift => Some(Delete(Deletion::CharForward)),
            "h" if !shift => Some(Delete(Deletion::CharBackward)),
            "k" if !shift => Some(Delete(Deletion::ParagraphForward)),
            "y" if !shift => Some(Yank),
            "t" if !shift => Some(Transpose),
            "o" if !shift && multi_line => Some(OpenLine),
            "backspace" => Some(Delete(Deletion::CharBackward)),
            _ => None,
        };
    }
    if ctrl && cmd && !alt && !shift && key == "space" {
        return Some(CharacterPalette);
    }
    if ctrl {
        return None;
    }
    match key {
        "left" => Some(Move(
            if cmd {
                LineStart
            } else if alt {
                WordLeft
            } else {
                Left
            },
            shift,
        )),
        "right" => Some(Move(
            if cmd {
                LineEnd
            } else if alt {
                WordRight
            } else {
                Right
            },
            shift,
        )),
        "up" if number && !cmd && !alt && !shift => Some(Step(1)),
        "down" if number && !cmd && !alt && !shift => Some(Step(-1)),
        "up" => Some(Move(
            if cmd {
                Start
            } else if alt {
                ParagraphBackward
            } else {
                Up
            },
            shift,
        )),
        "down" => Some(Move(
            if cmd {
                End
            } else if alt {
                ParagraphForward
            } else {
                Down
            },
            shift,
        )),
        "home" if !alt => Some(Move(if cmd { Start } else { LineStart }, shift)),
        "end" if !alt => Some(Move(if cmd { End } else { LineEnd }, shift)),
        "pageup" if !cmd && !alt => Some(Move(PageUp, shift)),
        "pagedown" if !cmd && !alt => Some(Move(PageDown, shift)),
        "backspace" => Some(Delete(if cmd {
            Deletion::LineBackward
        } else if alt {
            Deletion::WordBackward
        } else {
            Deletion::CharBackward
        })),
        "delete" => Some(Delete(if cmd {
            Deletion::LineForward
        } else if alt {
            Deletion::WordForward
        } else {
            Deletion::CharForward
        })),
        "enter" if !cmd => Some(if multi_line { LineBreak } else { Commit }),
        "a" if cmd && !alt && !shift => Some(SelectAll),
        "c" if cmd && !alt && !shift => Some(Copy),
        "x" if cmd && !alt && !shift => Some(Cut),
        "v" if cmd => Some(Paste),
        "z" if cmd && !alt => Some(if shift { Redo } else { Undo }),
        _ => None,
    }
}

fn other(
    keystroke: &Keystroke,
    os: OperatingSystem,
    multi_line: bool,
    number: bool,
) -> Option<Command> {
    use Command::*;
    use Movement::*;
    let modifiers = keystroke.modifiers;
    let (ctrl, alt, shift) = (modifiers.control, modifiers.alt, modifiers.shift);
    let windows = os == OperatingSystem::Windows;
    let linux = !windows;
    let key = keystroke.key.as_str();
    if modifiers.platform {
        return None;
    }
    if alt {
        // Alt-Backspace undoes on Windows. Alt and an arrow go back and forward.
        return (windows && key == "backspace" && !ctrl && !shift).then_some(Undo);
    }
    match key {
        "left" => Some(Move(if ctrl { WordLeft } else { Left }, shift)),
        "right" => Some(Move(
            match (ctrl, windows) {
                (false, _) => Right,
                (true, true) => NextWordStart,
                (true, false) => WordRight,
            },
            shift,
        )),
        "up" if number && !ctrl && !shift => Some(Step(1)),
        "down" if number && !ctrl && !shift => Some(Step(-1)),
        "up" if multi_line => Some(Move(if ctrl { ParagraphBackward } else { Up }, shift)),
        "down" if multi_line => Some(Move(if ctrl { ParagraphForward } else { Down }, shift)),
        "home" => Some(Move(if ctrl { Start } else { LineStart }, shift)),
        "end" => Some(Move(if ctrl { End } else { LineEnd }, shift)),
        "pageup" if !ctrl => Some(Move(PageUp, shift)),
        "pagedown" if !ctrl => Some(Move(PageDown, shift)),
        "backspace" => Some(Delete(if ctrl && shift && linux {
            Deletion::LineBackward
        } else if ctrl {
            Deletion::WordBackward
        } else {
            Deletion::CharBackward
        })),
        "delete" if shift && !ctrl => Some(Cut),
        "delete" => Some(Delete(match (ctrl, shift, windows) {
            (false, _, _) => Deletion::CharForward,
            (true, true, false) => Deletion::LineForward,
            (true, _, true) => Deletion::WordForwardToStart,
            (true, _, false) => Deletion::WordForward,
        })),
        "insert" if ctrl && !shift => Some(Copy),
        "insert" if shift && !ctrl => Some(Paste),
        "enter" if !ctrl => Some(if multi_line { LineBreak } else { Commit }),
        "a" if ctrl && !shift => Some(SelectAll),
        "c" if ctrl && !shift => Some(Copy),
        "x" if ctrl && !shift => Some(Cut),
        "v" if ctrl => Some(Paste),
        "z" if ctrl => Some(if shift { Redo } else { Undo }),
        "y" if ctrl && !shift => Some(Redo),
        _ => None,
    }
}

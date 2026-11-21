//! The text a field shows for its value: the value itself, or a dot for each
//! character of a password. Offsets map between the two, so the caret,
//! selections and clicks work on either.

use gpui::SharedString;
use unicode_segmentation::UnicodeSegmentation;

/// The dot a password shows for each character.
pub(crate) const MASK: char = '\u{2022}';

const MASK_LEN: usize = MASK.len_utf8();

/// The shown text and how its offsets map to the value's.
pub(crate) struct Display {
    pub text: SharedString,
    // For a password, the value offset each dot starts at, and the end.
    units: Option<Vec<usize>>,
}

impl Display {
    pub fn plain(value: &SharedString) -> Self {
        Self {
            text: value.clone(),
            units: None,
        }
    }

    pub fn masked(value: &str) -> Self {
        let mut units: Vec<usize> = value.grapheme_indices(true).map(|(index, _)| index).collect();
        let count = units.len();
        units.push(value.len());
        Self {
            text: MASK.to_string().repeat(count).into(),
            units: Some(units),
        }
    }

    /// The shown offset of a value offset.
    pub fn to_display(&self, offset: usize) -> usize {
        match &self.units {
            None => offset.min(self.text.len()),
            Some(units) => {
                let index = units.partition_point(|&unit| unit <= offset).saturating_sub(1);
                index * MASK_LEN
            }
        }
    }

    /// The value offset of a shown offset.
    pub fn to_value(&self, offset: usize) -> usize {
        match &self.units {
            None => offset.min(self.text.len()),
            Some(units) => {
                let index = (offset / MASK_LEN).min(units.len() - 1);
                units[index]
            }
        }
    }
}

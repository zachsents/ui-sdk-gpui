//! Port of `foundation/text/text-match.ts`, the `@ui-sdk/core/text-match`
//! entry: text matching the way the search components match, for your own
//! filters, sorting and highlights. Case and accents are ignored.

pub use crate::fuzzy::fuzzy_score;
pub use crate::r#match::{MatchMode, MatchRange, MatchSegment, match_ranges, match_segments};

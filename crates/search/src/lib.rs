//! A Rust port of [@codemirror/search](https://github.com/codemirror/search):
//! search cursors over documents, search queries (plain, whole-word and
//! regular expression), find and replace commands, match highlighting,
//! selection-match highlighting, and the search key bindings.
//!
//! Positions are byte offsets, as everywhere in this port. Regular
//! expressions use [`fancy_regex`] in place of JavaScript's engine.

mod cursor;
mod goto_line;
mod query;
mod regexp;
mod search;
mod selection_match;

pub use cursor::{CursorTest, Normalize, SearchCursor, SearchMatch};
pub use goto_line::goto_line_selection;
pub use query::{QueryCursor, QueryMatch, QueryTest, SearchQuery, SearchQueryConfig};
pub use regexp::{valid_regexp, RegExpCursor, RegExpCursorOptions, RegExpMatch, RegExpTest};
pub use search::{
    close_search_panel, find_next, find_previous, get_search_query, open_search_panel, replace_all, replace_next,
    search, search_keymap, search_panel_at_top, search_panel_open, select_matches, select_selection_matches,
    ResolvedSearchConfig, ScrollToMatch, SearchConfig, SearchHighlighter, SET_SEARCH_QUERY,
};
pub use selection_match::{highlight_selection_matches, select_next_occurrence, HighlightOptions, MatchHighlighter};

//! A Rust port of [`@codemirror/state`](https://codemirror.net/docs/ref/#state):
//! the editor state, transactions, document changes, selections, facets and
//! state fields.
//!
//! The design follows CodeMirror 6 closely, with these differences:
//!
//! - Positions are byte offsets into UTF-8 text, not UTF-16 code units.
//! - The document ([`Text`]) is a rope from the `crop` crate.
//! - Facets, fields, annotations and effects are typed handles over
//!   type-erased values. Define them once (for example in a `static`
//!   `LazyLock`) and reuse the handle.
//! - Invalid input (out-of-range positions, mismatched change sets) panics,
//!   where CodeMirror throws a `RangeError`.
//!
//! Ported from CodeMirror, copyright (C) 2018-2021 by Marijn Haverbeke
//! <marijn@haverbeke.berlin> and others, MIT license.

// The port stores many boxed closures; naming each type would not help.
#![allow(clippy::type_complexity)]

mod change;
mod char;
mod charcategory;
mod extension;
mod facet;
mod rangeset;
mod selection;
mod state;
mod text;
mod transaction;

pub use change::{ChangeDesc, ChangeSet, ChangeSpec, Insert, MapMode, RangeTouch};
pub use char::{count_column, find_cluster_break, find_column};
pub use charcategory::{make_categorizer, CharCategory};
pub use extension::{
    LanguageData, LanguageDataProvider, ALLOW_MULTIPLE_SELECTIONS, CHANGE_FILTER, LANGUAGE_DATA, LINE_SEPARATOR,
    PHRASES, READ_ONLY, TAB_SIZE, TRANSACTION_EXTENDER, TRANSACTION_FILTER,
};
pub use facet::{
    AnyValue, Compartment, CompartmentReconfigure, Dep, Extension, Facet, FacetSpec, Prec, StateField, StateFieldSpec,
};
pub use rangeset::{
    Range, RangeComparator, RangeCursor, RangeSet, RangeSetBuilder, RangeSetUpdate, RangeValue, SpanIterator,
};
pub use selection::{EditorSelection, SelectionRange};
pub use state::{ChangeByRange, DocInput, EditorState, EditorStateConfig, RangeChange, StateCommand};
pub use text::{Line, Text};
pub use transaction::{
    Annotation, AnnotationType, ChangeFilter, ChangeFilterResult, StateEffect, StateEffectType, Transaction,
    TransactionExtender, TransactionFilter, TransactionFilterResult, TransactionSpec, ADD_TO_HISTORY, APPEND_CONFIG,
    COMPARTMENT_RECONFIGURE, RECONFIGURE, REMOTE, TIME, USER_EVENT,
};

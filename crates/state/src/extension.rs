//! Built-in facets. Port of `extension.ts` and the facets defined on
//! `EditorState`.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use crate::facet::{AnyValue, Facet, FacetSpec};
use crate::state::EditorState;
use crate::transaction::{ChangeFilter, TransactionExtender, TransactionFilter};

/// Language data: named values (such as `"wordChars"` or
/// `"commentTokens"`) that apply at a position.
pub type LanguageData = Arc<HashMap<String, AnyValue>>;

/// A language data provider: given a state, a position and a side (-1, 0
/// or 1), return the language data objects that apply there.
pub type LanguageDataProvider = Arc<dyn Fn(&EditorState, usize, i32) -> Vec<LanguageData> + Send + Sync>;

/// A facet used to register [language data](EditorState::language_data_at)
/// providers.
pub static LANGUAGE_DATA: LazyLock<Facet<LanguageDataProvider>> = LazyLock::new(Facet::list);

/// When enabled, causes the editor to allow multiple ranges to be
/// selected.
pub static ALLOW_MULTIPLE_SELECTIONS: LazyLock<Facet<bool, bool>> = LazyLock::new(|| {
    Facet::define(FacetSpec::combine(|values: &[bool]| values.iter().any(|v| *v)).static_facet().compare_eq())
});

/// The line separator to use. By default, any of `"\n"`, `"\r\n"` and
/// `"\r"` is treated as a separator when splitting lines, and lines are
/// joined with `"\n"`.
///
/// When you configure a value here, that separator is used to split input
/// and to join lines on output. (Unlike CodeMirror, a lone `"\n"` in the
/// input is still a line break; the document always uses `"\n"`
/// internally.)
pub static LINE_SEPARATOR: LazyLock<Facet<String, Option<String>>> = LazyLock::new(|| {
    Facet::define(FacetSpec::combine(|values: &[String]| values.first().cloned()).static_facet().compare_eq())
});

/// Facet used to register change filters, which are called for each
/// transaction (unless explicitly disabled), and can suppress part of the
/// transaction's changes.
pub static CHANGE_FILTER: LazyLock<Facet<ChangeFilter>> = LazyLock::new(Facet::list);

/// Facet used to register a hook that gets a chance to update or replace
/// transaction specs before they are applied. This will only be applied for
/// transactions that don't have `filter` set to `false`.
pub static TRANSACTION_FILTER: LazyLock<Facet<TransactionFilter>> = LazyLock::new(Facet::list);

/// A more limited form of [`TRANSACTION_FILTER`], which can only add
/// annotations and effects. _But_, this type of filter runs even if the
/// transaction has disabled regular filtering. Extenders run _after_
/// filters, when both are present.
pub static TRANSACTION_EXTENDER: LazyLock<Facet<TransactionExtender>> = LazyLock::new(Facet::list);

/// Controls the value of [`EditorState::read_only`]. Defaults to false;
/// when its highest-precedence value is `true`, editing commands disable
/// themselves.
pub static READ_ONLY: LazyLock<Facet<bool, bool>> = LazyLock::new(|| {
    Facet::define(FacetSpec::combine(|values: &[bool]| values.first().copied().unwrap_or(false)).compare_eq())
});

/// Configures the tab size to use in this state. The first
/// (highest-precedence) value of the facet is used. If no value is given,
/// this defaults to 4.
pub static TAB_SIZE: LazyLock<Facet<usize, usize>> = LazyLock::new(|| {
    Facet::define(FacetSpec::combine(|values: &[usize]| values.first().copied().unwrap_or(4)).compare_eq())
});

/// Registers translation phrases. [`EditorState::phrase`] looks through all
/// maps registered with this facet to find translations for its argument.
pub static PHRASES: LazyLock<Facet<Arc<HashMap<String, String>>>> =
    LazyLock::new(|| Facet::define(FacetSpec::list().compare_input_eq()));

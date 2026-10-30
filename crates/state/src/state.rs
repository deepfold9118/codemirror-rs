//! The editor state. Port of `state.ts`.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::change::{ChangeSet, ChangeSpec, Insert};
use crate::char::find_cluster_break;
use crate::charcategory::{make_categorizer, CharCategory};
use crate::extension::{ALLOW_MULTIPLE_SELECTIONS, LANGUAGE_DATA, LINE_SEPARATOR, PHRASES, READ_ONLY, TAB_SIZE};
use crate::facet::{
    AnyValue, Configuration, ErasedFacet, Extension, Facet, StateField, STATUS_CHANGED, STATUS_COMPUTED,
    STATUS_COMPUTING,
};
use crate::selection::{check_selection, EditorSelection, SelectionRange};
use crate::text::Text;
use crate::transaction::{
    resolve_transaction, StateEffect, Transaction, TransactionSpec, APPEND_CONFIG, COMPARTMENT_RECONFIGURE, RECONFIGURE,
};

/// The initial document for [`EditorStateConfig`].
#[derive(Clone, Debug)]
pub enum DocInput {
    /// A string, split into lines with the configured line separator.
    Str(String),
    /// A ready-made document.
    Text(Text),
}

impl From<&str> for DocInput {
    fn from(s: &str) -> DocInput {
        DocInput::Str(s.to_string())
    }
}

impl From<String> for DocInput {
    fn from(s: String) -> DocInput {
        DocInput::Str(s)
    }
}

impl From<Text> for DocInput {
    fn from(t: Text) -> DocInput {
        DocInput::Text(t)
    }
}

/// Options passed when [creating](EditorState::create) an editor state.
#[derive(Clone, Default)]
pub struct EditorStateConfig {
    /// The initial document. Defaults to an empty document.
    pub doc: Option<DocInput>,
    /// The starting selection. Defaults to a cursor at the very start of
    /// the document.
    pub selection: Option<EditorSelection>,
    /// Extension(s) to associate with this state.
    pub extensions: Option<Extension>,
}

impl EditorStateConfig {
    /// An empty configuration.
    pub fn new() -> Self {
        EditorStateConfig::default()
    }

    /// Set the initial document.
    pub fn doc(mut self, doc: impl Into<DocInput>) -> Self {
        self.doc = Some(doc.into());
        self
    }

    /// Set the starting selection.
    pub fn selection(mut self, selection: EditorSelection) -> Self {
        self.selection = Some(selection);
        self
    }

    /// Set the extensions.
    pub fn extensions(mut self, extensions: impl Into<Extension>) -> Self {
        self.extensions = Some(extensions.into());
        self
    }
}

/// The result of a [`EditorState::change_by_range`] callback.
pub struct RangeChange {
    /// The updated range, in the coordinate space of the document produced
    /// by this callback's own changes.
    pub range: SelectionRange,
    /// Changes, in the coordinate space of the start document.
    pub changes: ChangeSpec,
    /// Effects to add.
    pub effects: Vec<StateEffect>,
}

impl RangeChange {
    /// A change with only an updated range.
    pub fn new(range: SelectionRange) -> RangeChange {
        RangeChange { range, changes: ChangeSpec::none(), effects: Vec::new() }
    }

    /// Set the changes.
    pub fn changes(mut self, changes: impl Into<ChangeSpec>) -> Self {
        self.changes = changes.into();
        self
    }
}

/// The combined result of [`EditorState::change_by_range`]. Converts into
/// a [`TransactionSpec`].
pub struct ChangeByRange {
    pub changes: ChangeSet,
    pub selection: EditorSelection,
    pub effects: Vec<StateEffect>,
}

impl From<ChangeByRange> for TransactionSpec {
    fn from(c: ChangeByRange) -> TransactionSpec {
        TransactionSpec {
            changes: Some(ChangeSpec::Set(c.changes)),
            selection: Some(c.selection),
            effects: c.effects,
            ..TransactionSpec::default()
        }
    }
}

/// Subtype of a command that only needs access to the state and a way to
/// dispatch transactions. In CodeMirror the target is `{state, dispatch}`;
/// an editor view passes its own state and dispatch function.
pub type StateCommand = fn(&EditorState, &mut dyn FnMut(Transaction)) -> bool;

#[derive(Clone)]
enum ComputeMode {
    Create,
    Update { tr: Transaction, prev: EditorState },
    Reconfigure { old: EditorState },
}

struct StateInner {
    config: Arc<Configuration>,
    doc: Text,
    selection: EditorSelection,
    values: Vec<OnceLock<AnyValue>>,
    status: Vec<AtomicU8>,
    compute: Mutex<Option<ComputeMode>>,
}

/// The editor state is a persistent (immutable) data structure. To update a
/// state, you [create](EditorState::update) a [transaction](Transaction),
/// which produces a _new_ state instance, without modifying the original.
///
/// Cloning a state is cheap.
#[derive(Clone)]
pub struct EditorState(Arc<StateInner>);

impl EditorState {
    fn build(
        config: Arc<Configuration>,
        doc: Text,
        selection: EditorSelection,
        mode: ComputeMode,
        tr: Option<&Transaction>,
    ) -> EditorState {
        let n = config.dynamic_slots.len();
        let state = EditorState(Arc::new(StateInner {
            config,
            doc,
            selection,
            values: (0..n).map(|_| OnceLock::new()).collect(),
            status: (0..n).map(|_| AtomicU8::new(0)).collect(),
            compute: Mutex::new(Some(mode)),
        }));
        // Fill in the computed state immediately, so that further queries
        // for it made during the update return this state.
        if let Some(tr) = tr {
            tr.set_state(state.clone());
        }
        for i in 0..n {
            state.ensure_addr(i << 1);
        }
        *state.0.compute.lock().unwrap() = None;
        state
    }

    pub(crate) fn config(&self) -> &Configuration {
        &self.0.config
    }

    pub(crate) fn config_arc(&self) -> &Arc<Configuration> {
        &self.0.config
    }

    pub(crate) fn ensure_addr(&self, addr: usize) -> u8 {
        if addr & 1 == 1 {
            return STATUS_COMPUTED;
        }
        let idx = addr >> 1;
        let status = self.0.status[idx].load(Ordering::Acquire);
        assert!(status != STATUS_COMPUTING, "Cyclic dependency between fields and/or facets");
        if status & STATUS_COMPUTED != 0 {
            return status;
        }
        self.0.status[idx].store(STATUS_COMPUTING, Ordering::Release);
        let mode = self.0.compute.lock().unwrap().clone().expect("state slots are computed during construction");
        let slot = &self.0.config.dynamic_slots[idx];
        let (value, changed) = match &mode {
            ComputeMode::Create => slot.create(self),
            ComputeMode::Update { tr, prev } => {
                let prev_value = prev.0.values[idx].get().expect("previous state is complete");
                slot.update(self, tr, prev_value)
            }
            ComputeMode::Reconfigure { old } => slot.reconfigure(self, old),
        };
        let _ = self.0.values[idx].set(value);
        let status = STATUS_COMPUTED | if changed { STATUS_CHANGED } else { 0 };
        self.0.status[idx].store(status, Ordering::Release);
        status
    }

    pub(crate) fn get_addr(&self, addr: usize) -> &AnyValue {
        if addr & 1 == 1 {
            &self.0.config.static_values[addr >> 1]
        } else {
            self.0.values[addr >> 1].get().expect("slot was computed")
        }
    }

    pub(crate) fn facet_erased<'a>(&'a self, facet: &'a dyn ErasedFacet) -> &'a AnyValue {
        match self.0.config.address.get(&facet.id()) {
            None => facet.default_value(),
            Some(&addr) => {
                self.ensure_addr(addr);
                self.get_addr(addr)
            }
        }
    }

    pub(crate) fn field_erased(&self, id: u64) -> Option<&AnyValue> {
        let &addr = self.0.config.address.get(&id)?;
        self.ensure_addr(addr);
        Some(self.get_addr(addr))
    }

    /// Whether two handles refer to the same state (CodeMirror's `===`).
    pub fn ptr_eq(&self, other: &EditorState) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// The current document.
    pub fn doc(&self) -> &Text {
        &self.0.doc
    }

    /// The current selection.
    pub fn selection(&self) -> &EditorSelection {
        &self.0.selection
    }

    /// Retrieve the value of a [state field](StateField).
    ///
    /// # Panics
    ///
    /// Panics when the state doesn't have that field; use
    /// [`field_opt`](EditorState::field_opt) to check.
    pub fn field<'a, V: Send + Sync + 'static>(&'a self, field: &StateField<V>) -> &'a V {
        self.field_opt(field).expect("Field is not present in this state")
    }

    /// Retrieve the value of a state field, or `None` when the state
    /// doesn't have it.
    pub fn field_opt<'a, V: Send + Sync + 'static>(&'a self, field: &StateField<V>) -> Option<&'a V> {
        self.field_erased(field.id()).map(|v| v.downcast_ref::<V>().expect("field value type"))
    }

    /// Get the value of a state [facet](Facet).
    pub fn facet<'a, I, O>(&'a self, facet: &'a Facet<I, O>) -> &'a O
    where
        I: Clone + Send + Sync + 'static,
        O: Send + Sync + 'static,
    {
        match self.0.config.address.get(&facet.id()) {
            None => facet.default_ref(),
            Some(&addr) => {
                self.ensure_addr(addr);
                self.get_addr(addr).downcast_ref::<O>().expect("facet output type")
            }
        }
    }

    /// Create a [transaction](Transaction) that updates this state.
    pub fn update(&self, spec: impl Into<TransactionSpec>) -> Transaction {
        resolve_transaction(self, vec![spec.into()], true)
    }

    /// Create a transaction from any number of specs. Unless
    /// [`sequential`](TransactionSpec::sequential) is set, the changes of
    /// each spec are assumed to start in the _current_ document (not the
    /// document produced by previous specs), and its selection and effects
    /// are assumed to refer to the document created by its _own_ changes.
    /// For selection, later specs take precedence over earlier ones.
    pub fn update_many(&self, specs: Vec<TransactionSpec>) -> Transaction {
        resolve_transaction(self, specs, true)
    }

    pub(crate) fn apply_transaction(&self, tr: &Transaction) {
        let mut conf: Option<&Arc<Configuration>> = Some(&self.0.config);
        let mut base = self.0.config.base.clone();
        let mut compartments = self.0.config.compartments.clone();
        for effect in tr.effects() {
            if let Some(r) = effect.value(&COMPARTMENT_RECONFIGURE) {
                conf = None;
                compartments.insert(r.compartment.id(), r.extension.clone());
            } else if let Some(ext) = effect.value(&RECONFIGURE) {
                conf = None;
                base = ext.clone();
            } else if let Some(ext) = effect.value(&APPEND_CONFIG) {
                conf = None;
                base = Extension::list([base, ext.clone()]);
            }
        }
        let (config, prev) = match conf {
            Some(conf) => (conf.clone(), tr.start_state().clone()),
            None => {
                let config = Arc::new(Configuration::resolve(base, &compartments, Some(self)));
                let intermediate = EditorState::build(
                    config.clone(),
                    self.0.doc.clone(),
                    self.0.selection.clone(),
                    ComputeMode::Reconfigure { old: self.clone() },
                    None,
                );
                (config, intermediate)
            }
        };
        let selection = if *tr.start_state().facet(&ALLOW_MULTIPLE_SELECTIONS) {
            tr.new_selection()
        } else {
            tr.new_selection().as_single()
        };
        EditorState::build(
            config,
            tr.new_doc().clone(),
            selection,
            ComputeMode::Update { tr: tr.clone(), prev },
            Some(tr),
        );
    }

    /// Create a transaction spec that replaces every selection range with
    /// the given content.
    pub fn replace_selection(&self, text: impl Into<Insert>) -> TransactionSpec {
        let text = match text.into() {
            Insert::Str(s) => self.to_text(&s),
            Insert::Text(t) => t,
        };
        self.change_by_range(|range| {
            RangeChange::new(EditorSelection::cursor_with(range.from + text.len(), -1, None, None))
                .changes(ChangeSpec::replace(range.from, range.to, text.clone()))
        })
        .into()
    }

    /// Create a set of changes and a new selection by running the given
    /// function for each range in the active selection. The function can
    /// return an optional set of changes (in the coordinate space of the
    /// start document), plus an updated range (in the coordinate space of
    /// the document produced by the call's own changes). This method merges
    /// all the changes and ranges into a single change set and selection.
    pub fn change_by_range(&self, mut f: impl FnMut(SelectionRange) -> RangeChange) -> ChangeByRange {
        let sel = self.selection();
        let first = f(sel.ranges[0]);
        let mut changes = self.changes(first.changes);
        let mut ranges = vec![first.range];
        let mut effects = first.effects;
        for i in 1..sel.ranges.len() {
            let result = f(sel.ranges[i]);
            let new_changes = self.changes(result.changes);
            let new_mapped = new_changes.map(changes.desc(), false);
            for range in ranges.iter_mut() {
                *range = range.map(new_mapped.desc(), -1);
            }
            let map_by = changes.map_desc(new_changes.desc(), true);
            ranges.push(result.range.map(&map_by, -1));
            changes = changes.compose(&new_mapped);
            let mut mapped = StateEffect::map_effects(&effects, new_mapped.desc());
            mapped.extend(StateEffect::map_effects(&result.effects, &map_by));
            effects = mapped;
        }
        ChangeByRange { changes, selection: EditorSelection::create(ranges, sel.main_index), effects }
    }

    /// Create a [change set](ChangeSet) from the given change description,
    /// taking the state's document length and line separator into account.
    pub fn changes(&self, spec: impl Into<ChangeSpec>) -> ChangeSet {
        match spec.into() {
            ChangeSpec::Set(set) => set,
            spec => ChangeSet::of(spec, self.0.doc.len(), self.facet(&LINE_SEPARATOR).as_deref()),
        }
    }

    /// Using the state's line separator, create a [`Text`] from the given
    /// string.
    pub fn to_text(&self, string: &str) -> Text {
        Text::from_str_with_separator(string, self.facet(&LINE_SEPARATOR).as_deref())
    }

    /// Return the given range of the document as a string, joined with the
    /// state's line break.
    pub fn slice_doc(&self, from: usize, to: usize) -> String {
        self.0.doc.slice_string(from, to, &self.line_break())
    }

    /// The whole document as a string, joined with the state's line break.
    pub fn doc_string(&self) -> String {
        self.slice_doc(0, self.0.doc.len())
    }

    /// The size (in columns) of a tab in the document, determined by the
    /// [`TAB_SIZE`] facet.
    pub fn tab_size(&self) -> usize {
        *self.facet(&TAB_SIZE)
    }

    /// The proper line-break string for this state.
    pub fn line_break(&self) -> String {
        self.facet(&LINE_SEPARATOR).clone().unwrap_or_else(|| "\n".to_string())
    }

    /// True when the editor is [configured](READ_ONLY) to be read-only.
    pub fn read_only(&self) -> bool {
        *self.facet(&READ_ONLY)
    }

    /// Look up a translation for the given phrase (via the [`PHRASES`]
    /// facet), or return the original string if no translation is found.
    ///
    /// Markers like `$1` (for the first value) and `$2` are replaced by the
    /// given values. A single `$` is equivalent to `$1`, and `$$` produces
    /// a literal dollar sign.
    pub fn phrase(&self, phrase: &str, insert: &[&str]) -> String {
        let mut phrase = phrase.to_string();
        for map in self.facet(&PHRASES) {
            if let Some(found) = map.get(&phrase) {
                phrase = found.clone();
                break;
            }
        }
        if insert.is_empty() {
            return phrase;
        }
        let mut out = String::with_capacity(phrase.len());
        let mut rest = phrase.as_str();
        while let Some(i) = rest.find('$') {
            out.push_str(&rest[..i]);
            let after = &rest[i + 1..];
            if let Some(stripped) = after.strip_prefix('$') {
                out.push('$');
                rest = stripped;
                continue;
            }
            let digits = after.bytes().take_while(u8::is_ascii_digit).count();
            let n: usize = if digits == 0 { 1 } else { after[..digits].parse().unwrap_or(0) };
            if n == 0 || n > insert.len() {
                out.push('$');
                out.push_str(&after[..digits]);
            } else {
                out.push_str(insert[n - 1]);
            }
            rest = &after[digits..];
        }
        out.push_str(rest);
        out
    }

    /// Find the values for a given language data field, provided by the
    /// [`LANGUAGE_DATA`] facet. Values of other types are skipped.
    pub fn language_data_at<T: Clone + 'static>(&self, name: &str, pos: usize, side: i32) -> Vec<T> {
        let mut values = Vec::new();
        for provider in self.facet(&LANGUAGE_DATA) {
            for result in provider(self, pos, side) {
                if let Some(v) = result.get(name).and_then(|v| v.downcast_ref::<T>()) {
                    values.push(v.clone());
                }
            }
        }
        values
    }

    /// Return a function that can categorize strings (expected to represent
    /// a single grapheme cluster) into word, space, or other characters.
    /// Word characters include those listed in the local language's
    /// `"wordChars"` language data (a `String`).
    pub fn char_categorizer(&self, at: usize) -> impl Fn(&str) -> CharCategory {
        let chars = self.language_data_at::<String>("wordChars", at, -1);
        let word_chars = chars.into_iter().next().unwrap_or_default();
        move |s: &str| make_categorizer(&word_chars)(s)
    }

    /// Find the word at the given position, meaning the range containing all
    /// word characters around it. If no word characters are adjacent to the
    /// position, this returns `None`.
    pub fn word_at(&self, pos: usize) -> Option<SelectionRange> {
        let line = self.0.doc.line_at(pos);
        let cat = self.char_categorizer(pos);
        let text = &line.text;
        let (mut start, mut end) = (pos - line.from, pos - line.from);
        while start > 0 {
            let prev = find_cluster_break(text, start, false, true);
            if cat(&text[prev..start]) != CharCategory::Word {
                break;
            }
            start = prev;
        }
        while end < line.length() {
            let next = find_cluster_break(text, end, true, true);
            if cat(&text[end..next]) != CharCategory::Word {
                break;
            }
            end = next;
        }
        if start == end {
            None
        } else {
            Some(EditorSelection::range(start + line.from, end + line.from))
        }
    }

    /// Create a new state. You'll usually only need this when initializing
    /// an editor—updated states are created by applying transactions.
    pub fn create(config: EditorStateConfig) -> EditorState {
        let configuration = Configuration::resolve(config.extensions.unwrap_or_default(), &Default::default(), None);
        let doc = match config.doc {
            None => Text::empty(),
            Some(DocInput::Text(t)) => t,
            Some(DocInput::Str(s)) => {
                Text::from_str_with_separator(&s, configuration.static_facet(&LINE_SEPARATOR).as_deref())
            }
        };
        let mut selection = config.selection.unwrap_or_else(|| EditorSelection::single(0, 0));
        check_selection(&selection, doc.len());
        if !*configuration.static_facet(&ALLOW_MULTIPLE_SELECTIONS) {
            selection = selection.as_single();
        }
        EditorState::build(Arc::new(configuration), doc, selection, ComputeMode::Create, None)
    }
}

impl Default for EditorState {
    fn default() -> Self {
        EditorState::create(EditorStateConfig::default())
    }
}

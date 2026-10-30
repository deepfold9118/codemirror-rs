//! Transactions, annotations and state effects. Port of `transaction.ts`.

use std::marker::PhantomData;
use std::sync::{Arc, LazyLock, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::change::{ChangeDesc, ChangeSet, ChangeSpec};
use crate::extension::{CHANGE_FILTER, LINE_SEPARATOR, TRANSACTION_EXTENDER, TRANSACTION_FILTER};
use crate::facet::{next_id, AnyValue, CompartmentReconfigure, Extension};
use crate::selection::{check_selection, EditorSelection};
use crate::state::EditorState;
use crate::text::Text;

// ---------- annotations ----------

/// Annotations are tagged values that are used to add metadata to
/// transactions in an extensible way. They should be used to model things
/// that affect the entire transaction (such as its time stamp or
/// information about its origin). For effects that happen _alongside_ the
/// other changes made by the transaction, [state effects](StateEffect) are
/// more appropriate.
#[derive(Clone)]
pub struct Annotation {
    type_id: u64,
    value: AnyValue,
}

/// Marker that identifies a type of [annotation](Annotation).
pub struct AnnotationType<T> {
    id: u64,
    _type: PhantomData<fn() -> T>,
}

impl<T> Clone for AnnotationType<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for AnnotationType<T> {}

impl<T: Send + Sync + 'static> AnnotationType<T> {
    /// Define a new type of annotation.
    pub fn define() -> Self {
        AnnotationType { id: next_id(), _type: PhantomData }
    }

    /// Create an instance of this annotation.
    pub fn of(&self, value: T) -> Annotation {
        Annotation { type_id: self.id, value: Arc::new(value) }
    }
}

impl Annotation {
    /// The value of this annotation, if it has the given type.
    pub fn value<'a, T: 'static>(&'a self, ty: &AnnotationType<T>) -> Option<&'a T> {
        if self.type_id == ty.id {
            self.value.downcast_ref::<T>()
        } else {
            None
        }
    }

    /// True when this annotation has the given type.
    pub fn is<T>(&self, ty: &AnnotationType<T>) -> bool {
        self.type_id == ty.id
    }
}

// ---------- effects ----------

type EffectMap = Arc<dyn Fn(&AnyValue, &ChangeDesc) -> Option<AnyValue> + Send + Sync>;

/// Representation of a type of state effect. Defined with
/// [`StateEffectType::define`].
pub struct StateEffectType<T> {
    id: u64,
    map: Option<EffectMap>,
    _type: PhantomData<fn() -> T>,
}

impl<T> Clone for StateEffectType<T> {
    fn clone(&self) -> Self {
        StateEffectType { id: self.id, map: self.map.clone(), _type: PhantomData }
    }
}

impl<T: Send + Sync + 'static> StateEffectType<T> {
    /// Define a new effect type whose values are not mapped through
    /// changes.
    pub fn define() -> Self {
        StateEffectType { id: next_id(), map: None, _type: PhantomData }
    }

    /// Define a new effect type that maps its values through position
    /// mappings. When `map` returns `None`, the mapping deletes the effect.
    pub fn define_with_map(map: impl Fn(&T, &ChangeDesc) -> Option<T> + Send + Sync + 'static) -> Self {
        let map: EffectMap = Arc::new(move |value: &AnyValue, mapping: &ChangeDesc| {
            map(value.downcast_ref::<T>().expect("effect value type"), mapping).map(|v| Arc::new(v) as AnyValue)
        });
        StateEffectType { id: next_id(), map: Some(map), _type: PhantomData }
    }

    /// Create a [state effect](StateEffect) instance of this type.
    pub fn of(&self, value: T) -> StateEffect {
        StateEffect { type_id: self.id, value: Arc::new(value), map: self.map.clone() }
    }
}

/// State effects can be used to represent additional effects associated
/// with a [transaction](Transaction::effects). They are often useful to
/// model changes to custom [state fields](crate::StateField), when those
/// changes aren't implicit in document or selection changes.
#[derive(Clone)]
pub struct StateEffect {
    type_id: u64,
    value: AnyValue,
    map: Option<EffectMap>,
}

impl StateEffect {
    /// Map this effect through a position mapping. Returns `None` when that
    /// ends up deleting the effect.
    pub fn map(&self, mapping: &ChangeDesc) -> Option<StateEffect> {
        match &self.map {
            None => Some(self.clone()),
            Some(map) => map(&self.value, mapping).map(|value| StateEffect { value, ..self.clone() }),
        }
    }

    /// Tells you whether this effect is of a given type.
    pub fn is<T>(&self, ty: &StateEffectType<T>) -> bool {
        self.type_id == ty.id
    }

    /// The effect's value, if it has the given type.
    pub fn value<'a, T: 'static>(&'a self, ty: &StateEffectType<T>) -> Option<&'a T> {
        if self.is(ty) {
            self.value.downcast_ref::<T>()
        } else {
            None
        }
    }

    /// Map a list of effects through a change set.
    pub fn map_effects(effects: &[StateEffect], mapping: &ChangeDesc) -> Vec<StateEffect> {
        effects.iter().filter_map(|e| e.map(mapping)).collect()
    }

    /// An effect that reconfigures the root extensions of the editor. Doing
    /// this will discard any extensions [appended](StateEffect::append_config),
    /// but does not reset the content of reconfigured compartments.
    pub fn reconfigure(ext: impl Into<Extension>) -> StateEffect {
        RECONFIGURE.of(ext.into())
    }

    /// An effect that appends extensions to the top-level configuration of
    /// the editor.
    pub fn append_config(ext: impl Into<Extension>) -> StateEffect {
        APPEND_CONFIG.of(ext.into())
    }
}

/// The effect type behind [`StateEffect::reconfigure`].
pub static RECONFIGURE: LazyLock<StateEffectType<Extension>> = LazyLock::new(StateEffectType::define);

/// The effect type behind [`StateEffect::append_config`].
pub static APPEND_CONFIG: LazyLock<StateEffectType<Extension>> = LazyLock::new(StateEffectType::define);

/// The effect type behind [`Compartment::reconfigure`](crate::Compartment::reconfigure).
pub static COMPARTMENT_RECONFIGURE: LazyLock<StateEffectType<CompartmentReconfigure>> =
    LazyLock::new(StateEffectType::define);

/// Annotation used to store transaction timestamps (milliseconds since the
/// Unix epoch). Automatically added to every transaction.
pub static TIME: LazyLock<AnnotationType<f64>> = LazyLock::new(AnnotationType::define);

/// Annotation used to associate a transaction with a user interface event.
/// Holds a string identifying the event, using a dot-separated format to
/// support attaching more specific information. The events used by the core
/// libraries are:
///
///  - `"input"` when content is entered
///    - `"input.type"` for typed input
///      - `"input.type.compose"` for composition
///    - `"input.paste"` for pasted input
///    - `"input.drop"` when adding content with drag-and-drop
///    - `"input.complete"` when autocompleting
///  - `"delete"` when the user deletes content
///    - `"delete.selection"` when deleting the selection
///    - `"delete.forward"` when deleting forward from the selection
///    - `"delete.backward"` when deleting backward from the selection
///    - `"delete.cut"` when cutting to the clipboard
///  - `"move"` when content is moved
///    - `"move.drop"` when content is moved within the editor through
///      drag-and-drop
///  - `"select"` when explicitly changing the selection
///    - `"select.pointer"` when selecting with a mouse or other pointing
///      device
///  - `"undo"` and `"redo"` for history actions
///
/// Use [`Transaction::is_user_event`] to check whether the annotation
/// matches a given event.
pub static USER_EVENT: LazyLock<AnnotationType<String>> = LazyLock::new(AnnotationType::define);

/// Annotation indicating whether a transaction should be added to the undo
/// history or not.
pub static ADD_TO_HISTORY: LazyLock<AnnotationType<bool>> = LazyLock::new(AnnotationType::define);

/// Annotation indicating (when present and true) that a transaction
/// represents a change made by some other actor, not the user.
pub static REMOTE: LazyLock<AnnotationType<bool>> = LazyLock::new(AnnotationType::define);

// ---------- specs ----------

/// Describes a [transaction](Transaction) when calling
/// [`EditorState::update`].
#[derive(Clone, Default)]
pub struct TransactionSpec {
    /// The changes to the document made by this transaction.
    pub changes: Option<ChangeSpec>,
    /// When set, this transaction explicitly updates the selection. Offsets
    /// in this selection should refer to the document as it is _after_ the
    /// transaction.
    pub selection: Option<EditorSelection>,
    /// State effects attached to this transaction. When they contain
    /// positions and this same spec makes changes, those positions should
    /// refer to positions in the updated document.
    pub effects: Vec<StateEffect>,
    /// Annotations for this transaction.
    pub annotations: Vec<Annotation>,
    /// Shorthand for a [`USER_EVENT`] annotation.
    pub user_event: Option<String>,
    /// When `true`, the transaction is marked as needing to scroll the
    /// current selection into view.
    pub scroll_into_view: bool,
    /// By default, transactions can be modified by change filters and
    /// transaction filters. Set this to `Some(false)` to disable that.
    pub filter: Option<bool>,
    /// Normally, when multiple specs are combined (for example by
    /// [`EditorState::update_many`]), the positions in `changes` are taken
    /// to refer to the document positions in the initial document. When a
    /// spec has `sequential` set to true, its positions will be taken to
    /// refer to the document created by the specs before it instead.
    pub sequential: bool,
}

impl TransactionSpec {
    /// An empty spec.
    pub fn new() -> Self {
        TransactionSpec::default()
    }

    /// Set the changes.
    pub fn changes(mut self, changes: impl Into<ChangeSpec>) -> Self {
        self.changes = Some(changes.into());
        self
    }

    /// Set the selection.
    pub fn selection(mut self, selection: EditorSelection) -> Self {
        self.selection = Some(selection);
        self
    }

    /// Add an effect.
    pub fn effect(mut self, effect: StateEffect) -> Self {
        self.effects.push(effect);
        self
    }

    /// Add an annotation.
    pub fn annotation(mut self, annotation: Annotation) -> Self {
        self.annotations.push(annotation);
        self
    }

    /// Set the user event.
    pub fn user_event(mut self, event: impl Into<String>) -> Self {
        self.user_event = Some(event.into());
        self
    }

    /// Scroll the selection into view.
    pub fn scroll_into_view(mut self) -> Self {
        self.scroll_into_view = true;
        self
    }

    /// Disable change and transaction filters for this spec.
    pub fn no_filter(mut self) -> Self {
        self.filter = Some(false);
        self
    }

    /// Interpret this spec's positions in the document produced by the
    /// specs before it.
    pub fn sequential(mut self) -> Self {
        self.sequential = true;
        self
    }
}

impl From<&Transaction> for TransactionSpec {
    fn from(tr: &Transaction) -> TransactionSpec {
        TransactionSpec {
            changes: Some(ChangeSpec::Set(tr.changes().clone())),
            selection: tr.selection().cloned(),
            effects: tr.effects().to_vec(),
            annotations: tr.0.annotations.clone(),
            user_event: None,
            scroll_into_view: tr.scroll_into_view(),
            filter: None,
            sequential: false,
        }
    }
}

/// What a [change filter](crate::CHANGE_FILTER) decides.
#[derive(Clone, Debug)]
pub enum ChangeFilterResult {
    /// Keep all changes.
    Allow,
    /// Drop all changes.
    Block,
    /// Suppress changes in these ranges, given as pairs of positions (so
    /// `[10, 20, 100, 110]` suppresses changes between 10 and 20, and
    /// between 100 and 110).
    Ranges(Vec<usize>),
}

/// What a [transaction filter](crate::TRANSACTION_FILTER) returns.
#[derive(Clone)]
pub enum TransactionFilterResult {
    /// Keep the transaction as it is.
    Keep,
    /// Replace it with another transaction.
    Transaction(Transaction),
    /// Replace it with these specs, combined as in
    /// [`EditorState::update_many`]. Use `TransactionSpec::from(tr)` to
    /// include the original transaction.
    Specs(Vec<TransactionSpec>),
}

/// A change filter function.
pub type ChangeFilter = Arc<dyn Fn(&Transaction) -> ChangeFilterResult + Send + Sync>;
/// A transaction filter function.
pub type TransactionFilter = Arc<dyn Fn(&Transaction) -> TransactionFilterResult + Send + Sync>;
/// A transaction extender function. Only the `effects` and `annotations`
/// of the returned spec are used.
pub type TransactionExtender = Arc<dyn Fn(&Transaction) -> Option<TransactionSpec> + Send + Sync>;

// ---------- transactions ----------

struct TransactionInner {
    start_state: EditorState,
    changes: ChangeSet,
    selection: Option<EditorSelection>,
    effects: Vec<StateEffect>,
    annotations: Vec<Annotation>,
    scroll_into_view: bool,
    doc: OnceLock<Text>,
    state: OnceLock<EditorState>,
}

/// Changes to the editor state are grouped into transactions. Typically, a
/// user action creates a single transaction, which may contain any number
/// of document changes, may change the selection, or have other effects.
/// Create a transaction by calling [`EditorState::update`].
///
/// Cloning a transaction is cheap and keeps its identity.
#[derive(Clone)]
pub struct Transaction(Arc<TransactionInner>);

fn now_millis() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64() * 1000.0).unwrap_or(0.0)
}

impl Transaction {
    pub(crate) fn create(
        start_state: EditorState,
        changes: ChangeSet,
        selection: Option<EditorSelection>,
        effects: Vec<StateEffect>,
        mut annotations: Vec<Annotation>,
        scroll_into_view: bool,
    ) -> Transaction {
        if let Some(selection) = &selection {
            check_selection(selection, changes.new_length());
        }
        if !annotations.iter().any(|a| a.is(&TIME)) {
            annotations.push(TIME.of(now_millis()));
        }
        Transaction(Arc::new(TransactionInner {
            start_state,
            changes,
            selection,
            effects,
            annotations,
            scroll_into_view,
            doc: OnceLock::new(),
            state: OnceLock::new(),
        }))
    }

    /// The state from which the transaction starts.
    pub fn start_state(&self) -> &EditorState {
        &self.0.start_state
    }

    /// The document changes made by this transaction.
    pub fn changes(&self) -> &ChangeSet {
        &self.0.changes
    }

    /// The selection set by this transaction, or `None` if it doesn't
    /// explicitly set a selection.
    pub fn selection(&self) -> Option<&EditorSelection> {
        self.0.selection.as_ref()
    }

    /// The effects added to the transaction.
    pub fn effects(&self) -> &[StateEffect] {
        &self.0.effects
    }

    /// Whether the selection should be scrolled into view after this
    /// transaction is dispatched.
    pub fn scroll_into_view(&self) -> bool {
        self.0.scroll_into_view
    }

    /// The new document produced by the transaction. Unlike
    /// [`state`](Transaction::state)`.doc()`, accessing this won't force the
    /// entire new state to be computed right away, so transaction filters
    /// should use this when they need to look at the new document.
    pub fn new_doc(&self) -> &Text {
        self.0.doc.get_or_init(|| self.0.changes.apply(self.0.start_state.doc()))
    }

    /// The new selection produced by the transaction. If
    /// [`selection`](Transaction::selection) is `None`, this maps the start
    /// state's current selection through the changes made by the
    /// transaction.
    pub fn new_selection(&self) -> EditorSelection {
        match &self.0.selection {
            Some(sel) => sel.clone(),
            None => self.0.start_state.selection().map(self.0.changes.desc(), -1),
        }
    }

    /// The new state created by the transaction. Computed on demand (but
    /// retained for subsequent access), so avoid it in transaction filters
    /// when possible.
    pub fn state(&self) -> &EditorState {
        if self.0.state.get().is_none() {
            self.0.start_state.apply_transaction(self);
        }
        self.0.state.get().expect("applying a transaction sets its state")
    }

    pub(crate) fn set_state(&self, state: EditorState) {
        let _ = self.0.state.set(state);
    }

    /// Get the value of the given annotation type, if any.
    pub fn annotation<'a, T: 'static>(&'a self, ty: &AnnotationType<T>) -> Option<&'a T> {
        self.0.annotations.iter().find_map(|a| a.value(ty))
    }

    /// All annotations of this transaction.
    pub fn annotations(&self) -> &[Annotation] {
        &self.0.annotations
    }

    /// Indicates whether the transaction changed the document.
    pub fn doc_changed(&self) -> bool {
        !self.0.changes.is_empty()
    }

    /// Indicates whether this transaction reconfigures the state (through a
    /// configuration compartment or with a top-level configuration effect).
    pub fn reconfigured(&self) -> bool {
        !Arc::ptr_eq(self.0.start_state.config_arc(), self.state().config_arc())
    }

    /// Returns true if the transaction has a [user event](USER_EVENT)
    /// annotation that is equal to or more specific than `event`. For
    /// example, if the transaction has `"select.pointer"` as user event,
    /// `"select"` and `"select.pointer"` will match it.
    pub fn is_user_event(&self, event: &str) -> bool {
        match self.annotation(&USER_EVENT) {
            None => false,
            Some(e) => {
                e == event || (e.len() > event.len() && e.starts_with(event) && e.as_bytes()[event.len()] == b'.')
            }
        }
    }
}

fn join_ranges(a: &[usize], b: &[usize]) -> Vec<usize> {
    let mut result: Vec<usize> = Vec::new();
    let (mut ia, mut ib) = (0, 0);
    loop {
        let (from, to);
        if ia < a.len() && (ib == b.len() || b[ib] >= a[ia]) {
            from = a[ia];
            to = a[ia + 1];
            ia += 2;
        } else if ib < b.len() {
            from = b[ib];
            to = b[ib + 1];
            ib += 2;
        } else {
            return result;
        }
        match result.last_mut() {
            Some(last) if *last >= from => {
                if *last < to {
                    *last = to;
                }
            }
            _ => result.extend([from, to]),
        }
    }
}

struct ResolvedSpec {
    changes: ChangeSet,
    selection: Option<EditorSelection>,
    effects: Vec<StateEffect>,
    annotations: Vec<Annotation>,
    scroll_into_view: bool,
}

fn merge_transaction(a: ResolvedSpec, b: ResolvedSpec, sequential: bool) -> ResolvedSpec {
    let (map_for_a, map_for_b, changes);
    if sequential {
        map_for_a = b.changes.desc().clone();
        map_for_b = ChangeSet::empty(b.changes.length()).desc().clone();
        changes = a.changes.compose(&b.changes);
    } else {
        let mapped = b.changes.map(a.changes.desc(), false);
        map_for_b = a.changes.map_desc(b.changes.desc(), true);
        changes = a.changes.compose(&mapped);
        map_for_a = mapped.desc().clone();
    }
    let mut effects = StateEffect::map_effects(&a.effects, &map_for_a);
    effects.extend(StateEffect::map_effects(&b.effects, &map_for_b));
    let mut annotations = a.annotations;
    annotations.extend(b.annotations);
    ResolvedSpec {
        changes,
        selection: match b.selection {
            Some(sel) => Some(sel.map(&map_for_b, -1)),
            None => a.selection.map(|sel| sel.map(&map_for_a, -1)),
        },
        effects,
        annotations,
        scroll_into_view: a.scroll_into_view || b.scroll_into_view,
    }
}

fn resolve_transaction_inner(state: &EditorState, spec: TransactionSpec, doc_size: usize) -> ResolvedSpec {
    let mut annotations = spec.annotations;
    if let Some(event) = spec.user_event {
        annotations.push(USER_EVENT.of(event));
    }
    let changes = match spec.changes {
        Some(ChangeSpec::Set(set)) => set,
        changes => {
            ChangeSet::of(changes.unwrap_or_else(ChangeSpec::none), doc_size, state.facet(&LINE_SEPARATOR).as_deref())
        }
    };
    ResolvedSpec {
        changes,
        selection: spec.selection,
        effects: spec.effects,
        annotations,
        scroll_into_view: spec.scroll_into_view,
    }
}

pub(crate) fn resolve_transaction(state: &EditorState, specs: Vec<TransactionSpec>, mut filter: bool) -> Transaction {
    let mut specs = specs.into_iter();
    let first = specs.next().unwrap_or_default();
    if first.filter == Some(false) {
        filter = false;
    }
    let mut s = resolve_transaction_inner(state, first, state.doc().len());
    for spec in specs {
        if spec.filter == Some(false) {
            filter = false;
        }
        let seq = spec.sequential;
        let size = if seq { s.changes.new_length() } else { state.doc().len() };
        let resolved = resolve_transaction_inner(state, spec, size);
        s = merge_transaction(s, resolved, seq);
    }
    let tr = Transaction::create(state.clone(), s.changes, s.selection, s.effects, s.annotations, s.scroll_into_view);
    extend_transaction(if filter { filter_transaction(tr) } else { tr })
}

/// Finish a transaction by applying filters if necessary.
fn filter_transaction(mut tr: Transaction) -> Transaction {
    let state = tr.start_state().clone();

    // Change filters
    let mut result = ChangeFilterResult::Allow;
    for filter in state.facet(&CHANGE_FILTER) {
        match filter(&tr) {
            ChangeFilterResult::Allow => {}
            ChangeFilterResult::Block => {
                result = ChangeFilterResult::Block;
                break;
            }
            ChangeFilterResult::Ranges(ranges) => {
                result = match result {
                    ChangeFilterResult::Ranges(prev) => ChangeFilterResult::Ranges(join_ranges(&prev, &ranges)),
                    _ => ChangeFilterResult::Ranges(ranges),
                }
            }
        }
    }
    if !matches!(result, ChangeFilterResult::Allow) {
        let (changes, back) = match result {
            ChangeFilterResult::Ranges(ranges) => {
                let (changes, filtered) = tr.changes().filter(&ranges);
                let back = filtered.map_desc(changes.desc(), false).inverted_desc();
                (changes, back)
            }
            _ => (ChangeSet::empty(state.doc().len()), tr.changes().inverted_desc()),
        };
        tr = Transaction::create(
            state.clone(),
            changes,
            tr.selection().map(|s| s.map(&back, -1)),
            StateEffect::map_effects(tr.effects(), &back),
            tr.annotations().to_vec(),
            tr.scroll_into_view(),
        );
    }

    // Transaction filters
    let filters = state.facet(&TRANSACTION_FILTER);
    for filter in filters.iter().rev() {
        match filter(&tr) {
            TransactionFilterResult::Keep => {}
            TransactionFilterResult::Transaction(t) => tr = t,
            TransactionFilterResult::Specs(specs) => tr = resolve_transaction(&state, specs, false),
        }
    }
    tr
}

fn extend_transaction(tr: Transaction) -> Transaction {
    let state = tr.start_state().clone();
    let extenders = state.facet(&TRANSACTION_EXTENDER);
    let mut spec: Option<ResolvedSpec> = None;
    for extender in extenders.iter().rev() {
        let Some(extension) = extender(&tr) else { continue };
        if extension.effects.is_empty() && extension.annotations.is_empty() && extension.user_event.is_none() {
            continue;
        }
        let base = spec.take().unwrap_or_else(|| ResolvedSpec {
            changes: tr.changes().clone(),
            selection: tr.selection().cloned(),
            effects: tr.effects().to_vec(),
            annotations: tr.annotations().to_vec(),
            scroll_into_view: tr.scroll_into_view(),
        });
        let extra = TransactionSpec {
            effects: extension.effects,
            annotations: extension.annotations,
            user_event: extension.user_event,
            ..TransactionSpec::default()
        };
        let resolved = resolve_transaction_inner(&state, extra, tr.changes().new_length());
        spec = Some(merge_transaction(base, resolved, true));
    }
    match spec {
        None => tr,
        Some(spec) => Transaction::create(
            state,
            tr.changes().clone(),
            tr.selection().cloned(),
            spec.effects,
            spec.annotations,
            spec.scroll_into_view,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_ranges() {
        assert_eq!(join_ranges(&[0, 2], &[4, 6]), vec![0, 2, 4, 6]);
        assert_eq!(join_ranges(&[0, 4], &[2, 6]), vec![0, 6]);
        assert_eq!(join_ranges(&[5, 6], &[0, 1]), vec![0, 1, 5, 6]);
    }
}

//! The undo history. Port of `history.ts`.
//!
//! Not ported: JSON serialization of the history field, the `beforeinput`
//! handler for `historyUndo`/`historyRedo` events and the key bindings,
//! which belong to the view.

use std::sync::{Arc, LazyLock};

use codemirror_state::{
    AnnotationType, ChangeDesc, ChangeSet, EditorSelection, EditorState, Extension, Facet, FacetSpec, StateEffect,
    StateField, StateFieldSpec, Transaction, TransactionSpec, ADD_TO_HISTORY, TIME, USER_EVENT,
};

use crate::event_matches;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum BranchName {
    Done,
    Undone,
}

#[derive(Clone)]
struct FromHistory {
    side: BranchName,
    rest: Branch,
    selection: EditorSelection,
}

static FROM_HISTORY: LazyLock<AnnotationType<FromHistory>> = LazyLock::new(AnnotationType::define);

/// Values for the [`ISOLATE_HISTORY`] annotation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IsolateHistory {
    /// Prevent merging with previous transactions.
    Before,
    /// Prevent subsequent transactions from being combined with this one.
    After,
    /// Isolate the transaction on both sides.
    Full,
}

/// Transaction annotation that will prevent that transaction from being
/// combined with other transactions in the undo history.
pub static ISOLATE_HISTORY: LazyLock<AnnotationType<IsolateHistory>> = LazyLock::new(AnnotationType::define);

/// A function that, given a transaction, provides the effects the history
/// should store when inverting it.
pub type InvertedEffects = Arc<dyn Fn(&Transaction) -> Vec<StateEffect> + Send + Sync>;

/// This facet provides a way to register functions that, given a
/// transaction, provide a set of effects that the history should store when
/// inverting the transaction. This can be used to integrate some kinds of
/// effects in the history, so that they can be undone (and redone again).
pub static INVERTED_EFFECTS: LazyLock<Facet<InvertedEffects>> = LazyLock::new(Facet::list);

/// Configuration for [`history`].
#[derive(Clone, Default)]
pub struct HistoryConfig {
    /// The minimum depth (amount of events) to store. Defaults to 100.
    pub min_depth: Option<usize>,
    /// The maximum time (in milliseconds) that adjacent events can be apart
    /// and still be grouped together. Defaults to 500.
    pub new_group_delay: Option<f64>,
    /// By default, when close enough together in time, changes are joined
    /// into an existing undo event if they touch any of the changed ranges
    /// from that event. You can pass a custom predicate here to influence
    /// that logic.
    pub join_to_event: Option<Arc<dyn Fn(&Transaction, bool) -> bool + Send + Sync>>,
}

struct ResolvedConfig {
    min_depth: usize,
    new_group_delay: f64,
    join_to_event: Vec<Arc<dyn Fn(&Transaction, bool) -> bool + Send + Sync>>,
}

impl ResolvedConfig {
    fn join_to_event(&self, tr: &Transaction, is_adjacent: bool) -> bool {
        if self.join_to_event.is_empty() {
            is_adjacent
        } else {
            self.join_to_event.iter().any(|f| f(tr, is_adjacent))
        }
    }
}

static HISTORY_CONFIG: LazyLock<Facet<HistoryConfig, ResolvedConfig>> = LazyLock::new(|| {
    Facet::define(FacetSpec::combine(|configs: &[HistoryConfig]| ResolvedConfig {
        min_depth: configs.iter().filter_map(|c| c.min_depth).max().unwrap_or(100),
        new_group_delay: configs.iter().filter_map(|c| c.new_group_delay).reduce(f64::min).unwrap_or(500.0),
        join_to_event: configs.iter().filter_map(|c| c.join_to_event.clone()).collect(),
    }))
});

/// The state field used to store the history data.
pub static HISTORY_FIELD: LazyLock<StateField<HistoryState>> = LazyLock::new(|| {
    StateField::define(StateFieldSpec::new(
        |_| HistoryState::empty(),
        |state: &HistoryState, tr: &Transaction| update_history(state, tr),
    ))
});

fn update_history(state: &HistoryState, tr: &Transaction) -> HistoryState {
    let config = tr.state().facet(&HISTORY_CONFIG);

    if let Some(from_hist) = tr.annotation(&FROM_HISTORY) {
        let item = HistEvent::from_transaction(tr, Some(from_hist.selection.clone()));
        let from = from_hist.side;
        let mut other = if from == BranchName::Done { state.undone.clone() } else { state.done.clone() };
        other = match item {
            Some(item) => update_branch(&other, other.len(), config.min_depth, item),
            None => add_selection(&other, tr.start_state().selection().clone()),
        };
        return if from == BranchName::Done {
            HistoryState::new(from_hist.rest.clone(), other)
        } else {
            HistoryState::new(other, from_hist.rest.clone())
        };
    }

    let mut state = state.clone();
    let isolate = tr.annotation(&ISOLATE_HISTORY).copied();
    if matches!(isolate, Some(IsolateHistory::Full | IsolateHistory::Before)) {
        state = state.isolate();
    }

    if tr.annotation(&ADD_TO_HISTORY) == Some(&false) {
        return if !tr.changes().is_empty() { state.add_mapping(tr.changes().desc()) } else { state };
    }

    let event = HistEvent::from_transaction(tr, None);
    let time = tr.annotation(&TIME).copied().unwrap_or(0.0);
    let user_event = tr.annotation(&USER_EVENT).cloned();
    if let Some(event) = event {
        state = state.add_changes(event, time, user_event, config, tr);
    } else if tr.selection().is_some() {
        state = state.add_selection(tr.start_state().selection().clone(), time, user_event, config.new_group_delay);
    }

    if matches!(isolate, Some(IsolateHistory::Full | IsolateHistory::After)) {
        state = state.isolate();
    }
    state
}

/// Create a history extension with the given configuration.
pub fn history(config: HistoryConfig) -> Extension {
    Extension::list([Extension::from(&*HISTORY_FIELD), HISTORY_CONFIG.of(config)])
}

fn cmd(side: BranchName, selection: bool, state: &EditorState, dispatch: &mut dyn FnMut(Transaction)) -> bool {
    if !selection && state.read_only() {
        return false;
    }
    let Some(history_state) = state.field_opt(&HISTORY_FIELD) else { return false };
    let Some(tr) = history_state.pop(side, state, selection) else { return false };
    dispatch(tr);
    true
}

/// Undo a single group of history events. Returns false if no group was
/// available.
pub fn undo(state: &EditorState, dispatch: &mut dyn FnMut(Transaction)) -> bool {
    cmd(BranchName::Done, false, state, dispatch)
}

/// Redo a group of history events. Returns false if no group was available.
pub fn redo(state: &EditorState, dispatch: &mut dyn FnMut(Transaction)) -> bool {
    cmd(BranchName::Undone, false, state, dispatch)
}

/// Undo a change or selection change.
pub fn undo_selection(state: &EditorState, dispatch: &mut dyn FnMut(Transaction)) -> bool {
    cmd(BranchName::Done, true, state, dispatch)
}

/// Redo a change or selection change.
pub fn redo_selection(state: &EditorState, dispatch: &mut dyn FnMut(Transaction)) -> bool {
    cmd(BranchName::Undone, true, state, dispatch)
}

fn depth(side: BranchName, state: &EditorState) -> usize {
    let Some(hist) = state.field_opt(&HISTORY_FIELD) else { return 0 };
    let branch = if side == BranchName::Done { &hist.done } else { &hist.undone };
    branch.len() - usize::from(branch.first().is_some_and(|e| e.changes.is_none()))
}

/// The amount of undoable change events available in a given state.
pub fn undo_depth(state: &EditorState) -> usize {
    depth(BranchName::Done, state)
}

/// The amount of redoable change events available in a given state.
pub fn redo_depth(state: &EditorState) -> usize {
    depth(BranchName::Undone, state)
}

fn has_selection(side: BranchName, state: &EditorState) -> bool {
    let Some(hist) = state.field_opt(&HISTORY_FIELD) else { return false };
    let branch = if side == BranchName::Done { &hist.done } else { &hist.undone };
    branch.first().is_some_and(|e| !e.selections_after.is_empty())
}

/// Test whether there is an undoable selection available.
pub fn can_undo_selection(state: &EditorState) -> bool {
    has_selection(BranchName::Done, state)
}

/// Test whether there is a redoable selection available. (CodeMirror checks
/// the undo branch here, which looks like a slip; this port checks the redo
/// branch.)
pub fn can_redo_selection(state: &EditorState) -> bool {
    has_selection(BranchName::Undone, state)
}

// History events store groups of changes or effects that need to be
// undone/redone together.
struct HistEvent {
    // The changes in this event. Normal events hold at least one change or
    // effect. But it may be necessary to store selection events before the
    // first change, in which case a special type of instance is created
    // which doesn't hold any changes, with changes == start_selection ==
    // None.
    changes: Option<ChangeSet>,
    // The effects associated with this event.
    effects: Vec<StateEffect>,
    // Accumulated mapping (from ADD_TO_HISTORY == false) that should be
    // applied to events below this one.
    mapped: Option<ChangeDesc>,
    // The selection before this event.
    start_selection: Option<EditorSelection>,
    // Stores selection changes after this event, to be used for selection
    // undo/redo.
    selections_after: Vec<EditorSelection>,
}

impl HistEvent {
    fn set_sel_after(&self, after: Vec<EditorSelection>) -> HistEvent {
        HistEvent {
            changes: self.changes.clone(),
            effects: self.effects.clone(),
            mapped: self.mapped.clone(),
            start_selection: self.start_selection.clone(),
            selections_after: after,
        }
    }

    // This does not check ADD_TO_HISTORY and such, it assumes the
    // transaction needs to be converted to an item. Returns None when there
    // are no changes or effects in the transaction.
    fn from_transaction(tr: &Transaction, selection: Option<EditorSelection>) -> Option<Arc<HistEvent>> {
        let mut effects = Vec::new();
        for invert in tr.start_state().facet(&INVERTED_EFFECTS) {
            effects.extend(invert(tr));
        }
        if effects.is_empty() && tr.changes().is_empty() {
            return None;
        }
        Some(Arc::new(HistEvent {
            changes: Some(tr.changes().invert(tr.start_state().doc())),
            effects,
            mapped: None,
            start_selection: Some(selection.unwrap_or_else(|| tr.start_state().selection().clone())),
            selections_after: Vec::new(),
        }))
    }

    fn selection(selections: Vec<EditorSelection>) -> HistEvent {
        HistEvent {
            changes: None,
            effects: Vec::new(),
            mapped: None,
            start_selection: None,
            selections_after: selections,
        }
    }
}

type Branch = Vec<Arc<HistEvent>>;

fn update_branch(branch: &[Arc<HistEvent>], to: usize, max_len: usize, new_event: Arc<HistEvent>) -> Branch {
    let start = if to + 1 > max_len + 20 { to - max_len - 1 } else { 0 };
    let mut new_branch = branch[start..to].to_vec();
    new_branch.push(new_event);
    new_branch
}

fn is_adjacent(a: &ChangeDesc, b: &ChangeDesc) -> bool {
    let mut ranges = Vec::new();
    let mut adjacent = false;
    a.iter_changed_ranges(|f, t, _, _| ranges.push((f, t)), false);
    b.iter_changed_ranges(
        |_, _, f, t| {
            for &(from, to) in &ranges {
                if t >= from && f <= to {
                    adjacent = true;
                }
            }
        },
        false,
    );
    adjacent
}

fn eq_selection_shape(a: &EditorSelection, b: &EditorSelection) -> bool {
    a.ranges.len() == b.ranges.len() && a.ranges.iter().zip(&b.ranges).all(|(x, y)| x.is_empty() == y.is_empty())
}

const MAX_SELECTIONS_PER_EVENT: usize = 200;

fn add_selection(branch: &[Arc<HistEvent>], selection: EditorSelection) -> Branch {
    let Some(last_event) = branch.last() else {
        return vec![Arc::new(HistEvent::selection(vec![selection]))];
    };
    let after = &last_event.selections_after;
    let mut sels = after[after.len().saturating_sub(MAX_SELECTIONS_PER_EVENT)..].to_vec();
    if sels.last().is_some_and(|s| s.eq(&selection, false)) {
        return branch.to_vec();
    }
    sels.push(selection);
    update_branch(branch, branch.len() - 1, 1_000_000_000, Arc::new(last_event.set_sel_after(sels)))
}

// Assumes the top item has one or more selections_after values.
fn pop_selection(branch: &[Arc<HistEvent>]) -> Branch {
    let last = &branch[branch.len() - 1];
    let mut new_branch = branch.to_vec();
    let after = &last.selections_after;
    new_branch[branch.len() - 1] = Arc::new(last.set_sel_after(after[..after.len() - 1].to_vec()));
    new_branch
}

// Add a mapping to the top event in the given branch. If this maps away
// all the changes and effects in that item, drop it and propagate the
// mapping to the next item.
fn add_mapping_to_branch(branch: &[Arc<HistEvent>], mapping: &ChangeDesc) -> Branch {
    if branch.is_empty() {
        return Vec::new();
    }
    let mut length = branch.len();
    let mut selections = Vec::new();
    let mut mapping = mapping.clone();
    while length > 0 {
        let event = map_event(&branch[length - 1], &mapping, &selections);
        if event.changes.as_ref().is_some_and(|c| !c.is_empty()) || !event.effects.is_empty() {
            // Event survived mapping.
            let mut result = branch[..length].to_vec();
            result[length - 1] = Arc::new(event);
            return result;
        }
        // Drop this event, since there's no changes or effects left. (A
        // change-less event has no mapping, but it is always the bottom
        // event, so the loop ends here.)
        if let Some(mapped) = event.mapped {
            mapping = mapped;
        }
        length -= 1;
        selections = event.selections_after;
    }
    if selections.is_empty() {
        Vec::new()
    } else {
        vec![Arc::new(HistEvent::selection(selections))]
    }
}

fn map_event(event: &HistEvent, mapping: &ChangeDesc, extra_selections: &[EditorSelection]) -> HistEvent {
    let mut selections: Vec<EditorSelection> = event.selections_after.iter().map(|s| s.map(mapping, -1)).collect();
    selections.extend_from_slice(extra_selections);
    // Change-less events don't store mappings (they are always the last
    // event in a branch).
    let Some(changes) = &event.changes else { return HistEvent::selection(selections) };

    let mapped_changes = changes.map(mapping, false);
    let before = mapping.map_desc(changes, true);
    let full_mapping = match &event.mapped {
        Some(mapped) => mapped.compose_desc(&before),
        None => before.clone(),
    };
    HistEvent {
        changes: Some(mapped_changes),
        effects: StateEffect::map_effects(&event.effects, mapping),
        mapped: Some(full_mapping),
        start_selection: event.start_selection.as_ref().map(|s| s.map(&before, -1)),
        selections_after: selections,
    }
}

fn joinable_user_event(event: &str) -> bool {
    event_matches(event, "input.type") || event_matches(event, "delete")
}

/// The value of [`HISTORY_FIELD`]: the undo and redo branches.
#[derive(Clone)]
pub struct HistoryState {
    done: Branch,
    undone: Branch,
    prev_time: f64,
    prev_user_event: Option<String>,
}

impl HistoryState {
    fn new(done: Branch, undone: Branch) -> HistoryState {
        HistoryState { done, undone, prev_time: 0.0, prev_user_event: None }
    }

    fn empty() -> HistoryState {
        HistoryState::new(Vec::new(), Vec::new())
    }

    fn isolate(&self) -> HistoryState {
        if self.prev_time != 0.0 {
            HistoryState::new(self.done.clone(), self.undone.clone())
        } else {
            self.clone()
        }
    }

    fn add_changes(
        &self,
        event: Arc<HistEvent>,
        time: f64,
        user_event: Option<String>,
        config: &ResolvedConfig,
        tr: &Transaction,
    ) -> HistoryState {
        let mut done = self.done.clone();
        let last_event = done.last().cloned();
        let join = match (&last_event, &event.changes) {
            (Some(last), Some(changes)) => {
                last.changes.as_ref().is_some_and(|c| !c.is_empty())
                    && user_event.as_deref().is_none_or(joinable_user_event)
                    && ((last.selections_after.is_empty()
                        && time - self.prev_time < config.new_group_delay
                        && config.join_to_event(tr, is_adjacent(last.changes.as_ref().unwrap(), changes)))
                        // For compose (but not compose.start) events, always
                        // join with previous event.
                        || user_event.as_deref() == Some("input.type.compose"))
            }
            _ => false,
        };
        if join {
            let last = last_event.unwrap();
            let last_changes = last.changes.as_ref().unwrap();
            let mut effects = StateEffect::map_effects(&event.effects, last_changes.desc());
            effects.extend(last.effects.iter().cloned());
            let joined = HistEvent {
                changes: Some(event.changes.as_ref().unwrap().compose(last_changes)),
                effects,
                mapped: last.mapped.clone(),
                start_selection: last.start_selection.clone(),
                selections_after: Vec::new(),
            };
            done = update_branch(&done, done.len() - 1, config.min_depth, Arc::new(joined));
        } else {
            done = update_branch(&done, done.len(), config.min_depth, event);
        }
        HistoryState { done, undone: Vec::new(), prev_time: time, prev_user_event: user_event }
    }

    fn add_selection(
        &self,
        selection: EditorSelection,
        time: f64,
        user_event: Option<String>,
        new_group_delay: f64,
    ) -> HistoryState {
        let last: &[EditorSelection] = self.done.last().map_or(&[], |e| &e.selections_after);
        if let Some(last_sel) = last.last() {
            if time - self.prev_time < new_group_delay
                && user_event == self.prev_user_event
                && user_event.as_deref().is_some_and(|e| event_matches(e, "select"))
                && eq_selection_shape(last_sel, &selection)
            {
                return self.clone();
            }
        }
        HistoryState {
            done: add_selection(&self.done, selection),
            undone: self.undone.clone(),
            prev_time: time,
            prev_user_event: user_event,
        }
    }

    fn add_mapping(&self, mapping: &ChangeDesc) -> HistoryState {
        HistoryState {
            done: add_mapping_to_branch(&self.done, mapping),
            undone: add_mapping_to_branch(&self.undone, mapping),
            prev_time: self.prev_time,
            prev_user_event: self.prev_user_event.clone(),
        }
    }

    fn pop(&self, side: BranchName, state: &EditorState, only_selection: bool) -> Option<Transaction> {
        let branch = if side == BranchName::Done { &self.done } else { &self.undone };
        let event = branch.last()?;
        let selection = match (event.selections_after.first(), &event.start_selection) {
            (Some(first), _) => first.clone(),
            (None, Some(start)) => start.map(&event.changes.as_ref().unwrap().inverted_desc(), 1),
            (None, None) => state.selection().clone(),
        };
        if only_selection && !event.selections_after.is_empty() {
            Some(
                state.update(
                    TransactionSpec::new()
                        .selection(event.selections_after.last().unwrap().clone())
                        .annotation(FROM_HISTORY.of(FromHistory { side, rest: pop_selection(branch), selection }))
                        .user_event(if side == BranchName::Done { "select.undo" } else { "select.redo" })
                        .scroll_into_view(),
                ),
            )
        } else {
            let changes = event.changes.as_ref()?;
            let mut rest = branch[..branch.len() - 1].to_vec();
            if let Some(mapped) = &event.mapped {
                rest = add_mapping_to_branch(&rest, mapped);
            }
            let mut spec = TransactionSpec::new()
                .changes(changes.clone())
                .annotation(FROM_HISTORY.of(FromHistory { side, rest, selection }))
                .no_filter()
                .user_event(if side == BranchName::Done { "undo" } else { "redo" })
                .scroll_into_view();
            spec.selection = event.start_selection.clone();
            spec.effects = event.effects.clone();
            Some(state.update(spec))
        }
    }
}

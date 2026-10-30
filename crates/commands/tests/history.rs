//! Port of CodeMirror's `test/test-history.ts`. The JSON serialization test
//! is not ported (JSON is not ported yet).

mod common;

use std::sync::Arc;

use codemirror_commands::{
    history, redo, redo_depth, redo_selection, undo, undo_depth, undo_selection, HistoryConfig, IsolateHistory,
    INVERTED_EFFECTS, ISOLATE_HISTORY,
};
use codemirror_state::{
    ChangeDesc, ChangeSpec, EditorSelection, EditorState, EditorStateConfig, Extension, StateCommand, StateEffectType,
    StateField, StateFieldSpec, Text, Transaction, TransactionSpec, ADD_TO_HISTORY, ALLOW_MULTIPLE_SELECTIONS, TIME,
};

fn mk_state(config: HistoryConfig, doc: &str) -> EditorState {
    EditorState::create(
        EditorStateConfig::new().doc(doc).extensions(vec![history(config), ALLOW_MULTIPLE_SELECTIONS.of(true)]),
    )
}

fn mk() -> EditorState {
    mk_state(HistoryConfig::default(), "")
}

fn update(state: &EditorState, spec: TransactionSpec) -> EditorState {
    state.update(spec).state().clone()
}

fn type_at(state: &EditorState, text: &str, at: usize) -> EditorState {
    update(state, TransactionSpec::new().changes(ChangeSpec::insert(at, text)))
}

fn type_(state: &EditorState, text: &str) -> EditorState {
    type_at(state, text, state.doc().len())
}

fn timed_type(state: &EditorState, text: &str, at_time: f64) -> EditorState {
    update(
        state,
        TransactionSpec::new().changes(ChangeSpec::insert(state.doc().len(), text)).annotation(TIME.of(at_time)),
    )
}

fn receive(state: &EditorState, text: &str, from: usize, to: usize) -> EditorState {
    update(
        state,
        TransactionSpec::new().changes(ChangeSpec::replace(from, to, text)).annotation(ADD_TO_HISTORY.of(false)),
    )
}

fn isolate(state: &EditorState, how: IsolateHistory) -> EditorState {
    update(state, TransactionSpec::new().annotation(ISOLATE_HISTORY.of(how)))
}

fn select(state: &EditorState, anchor: usize, head: usize) -> EditorState {
    update(state, TransactionSpec::new().selection(EditorSelection::single(anchor, head)))
}

fn command_ok(state: &EditorState, cmd: StateCommand, success: bool) -> EditorState {
    let mut result = state.clone();
    assert_eq!(cmd(state, &mut |tr: Transaction| result = tr.state().clone()), success);
    result
}

fn command(state: &EditorState, cmd: StateCommand) -> EditorState {
    command_ok(state, cmd, true)
}

fn doc(state: &EditorState) -> String {
    state.doc_string()
}

#[test]
fn allows_to_undo_a_change() {
    let mut state = type_(&mk(), "newtext");
    state = command(&state, undo);
    assert_eq!(doc(&state), "");
}

#[test]
fn allows_to_undo_nearby_changes_in_one_change() {
    let mut state = type_(&type_(&mk(), "new"), "text");
    state = command(&state, undo);
    assert_eq!(doc(&state), "");
}

#[test]
fn allows_to_redo_a_change() {
    let mut state = type_(&mk(), "newtext");
    state = command(&command(&state, undo), redo);
    assert_eq!(doc(&state), "newtext");
}

#[test]
fn allows_to_redo_nearby_changes_in_one_change() {
    let mut state = type_(&type_(&mk(), "new"), "text");
    state = command(&command(&state, undo), redo);
    assert_eq!(doc(&state), "newtext");
}

#[test]
fn puts_the_cursor_after_the_change_on_redo() {
    let mut state = mk_state(HistoryConfig::default(), "one\n\ntwo");
    state = update(
        &state,
        TransactionSpec::new().changes(ChangeSpec::insert(3, "!")).selection(EditorSelection::single(4, 4)),
    );
    let len = state.doc().len();
    state = select(&state, len, len);
    state = command(&command(&state, undo), redo);
    assert_eq!(state.selection().main().head(), 4);
}

#[test]
fn restores_selection_on_undo_redo_undo() {
    let mut state = mk_state(HistoryConfig::default(), "1\n2\n3");
    state = select(&state, 5, 5);
    state = update(
        &state,
        TransactionSpec::new().changes(ChangeSpec::insert(5, ".")).selection(EditorSelection::single(6, 6)),
    );
    state = select(&state, 0, 0);
    state = command(&state, undo);
    assert_eq!(state.selection().main().head(), 5);
    state = select(&state, 0, 0);
    state = command(&state, redo);
    assert_eq!(state.selection().main().head(), 6);
    state = select(&state, 0, 0);
    state = command(&state, undo);
    assert_eq!(state.selection().main().head(), 5);
}

#[test]
fn tracks_multiple_levels_of_history() {
    let mut state = mk_state(HistoryConfig::default(), "one");
    state = type_(&state, "new");
    state = type_(&state, "text");
    state = type_at(&state, "some", 0);
    assert_eq!(doc(&state), "someonenewtext");
    state = command(&state, undo);
    assert_eq!(doc(&state), "onenewtext");
    state = command(&state, undo);
    assert_eq!(doc(&state), "one");
    state = command(&state, redo);
    assert_eq!(doc(&state), "onenewtext");
    state = command(&state, redo);
    assert_eq!(doc(&state), "someonenewtext");
    state = command(&state, undo);
    assert_eq!(doc(&state), "onenewtext");
}

#[test]
fn starts_a_new_event_when_new_group_delay_elapses() {
    let mut state = mk_state(HistoryConfig { new_group_delay: Some(1000.0), ..Default::default() }, "");
    state = timed_type(&state, "a", 1000.0);
    state = timed_type(&state, "b", 1600.0);
    assert_eq!(undo_depth(&state), 1);
    state = timed_type(&state, "c", 2700.0);
    assert_eq!(undo_depth(&state), 2);
    state = command(&state, undo);
    state = timed_type(&state, "d", 2800.0);
    assert_eq!(undo_depth(&state), 2);
}

#[test]
fn supports_a_custom_join_predicate() {
    let config = HistoryConfig {
        join_to_event: Some(Arc::new(|tr: &Transaction, adj: bool| {
            if !adj {
                return false;
            }
            let mut space = false;
            tr.changes().iter_changes(
                |_, _, _, _, text: Text| {
                    if !text.is_empty() && text.slice_string(0, 1, "\n") == " " {
                        space = true;
                    }
                },
                false,
            );
            !space
        })),
        ..Default::default()
    };
    let mut state = mk_state(config, "");
    for ch in ["a", "b", " ", "c", "d"] {
        state = type_(&state, ch);
    }
    assert_eq!(doc(&state), "ab cd");
    state = command(&state, undo);
    assert_eq!(doc(&state), "ab");
    state = command(&state, undo);
    assert_eq!(doc(&state), "");
}

#[test]
fn allows_changes_that_arent_part_of_the_history() {
    let mut state = type_(&mk(), "hello");
    state = receive(&state, "oops", 0, 0);
    state = receive(&state, "!", 9, 9);
    state = command(&state, undo);
    assert_eq!(doc(&state), "oops!");
}

#[test]
fn doesnt_get_confused_by_an_undo_not_adding_any_redo_item() {
    let mut state = mk_state(HistoryConfig::default(), "ab");
    state = type_at(&state, "cd", 1);
    state = receive(&state, "123", 0, 4);
    state = command_ok(&state, undo, false);
    command_ok(&state, redo, false);
}

#[test]
fn accurately_maps_changes_through_each_other() {
    let mut state = mk_state(HistoryConfig::default(), "123");
    state = update(
        &state,
        TransactionSpec::new().changes(vec![
            ChangeSpec::replace(0, 1, "ab"),
            ChangeSpec::replace(1, 2, "cd"),
            ChangeSpec::replace(2, 3, "ef"),
        ]),
    );
    state = receive(&state, "!!!!!!!!", 2, 2);
    state = command(&command(&state, undo), redo);
    assert_eq!(doc(&state), "ab!!!!!!!!cdef");
}

#[test]
fn can_handle_complex_editing_sequences() {
    let mut state = type_(&mk(), "hello");
    state = isolate(&state, IsolateHistory::Before);
    state = type_(&state, "!");
    state = receive(&state, "....", 0, 0);
    state = type_at(&state, "\n\n", 2);
    assert_eq!(doc(&state), "..\n\n..hello!");
    state = receive(&state, "\n\n", 1, 1);
    state = command(&command(&state, undo), undo);
    assert_eq!(doc(&state), ".\n\n...hello");
    state = command(&state, undo);
    assert_eq!(doc(&state), ".\n\n...");
}

#[test]
fn supports_overlapping_edits() {
    let mut state = type_(&mk(), "hello");
    state = isolate(&state, IsolateHistory::Before);
    state = update(&state, TransactionSpec::new().changes(ChangeSpec::delete(0, 5)));
    assert_eq!(doc(&state), "");
    state = command(&state, undo);
    assert_eq!(doc(&state), "hello");
    state = command(&state, undo);
    assert_eq!(doc(&state), "");
}

#[test]
fn supports_overlapping_edits_that_arent_collapsed() {
    let mut state = receive(&mk(), "h", 0, 0);
    state = type_(&state, "ello");
    state = isolate(&state, IsolateHistory::Before);
    state = update(&state, TransactionSpec::new().changes(ChangeSpec::delete(0, 5)));
    assert_eq!(doc(&state), "");
    state = command(&state, undo);
    assert_eq!(doc(&state), "hello");
    state = command(&state, undo);
    assert_eq!(doc(&state), "h");
}

#[test]
fn supports_overlapping_unsynced_deletes() {
    let mut state = type_(&mk(), "hi");
    state = isolate(&state, IsolateHistory::Before);
    state = type_(&state, "hello");
    state =
        update(&state, TransactionSpec::new().changes(ChangeSpec::delete(0, 7)).annotation(ADD_TO_HISTORY.of(false)));
    assert_eq!(doc(&state), "");
    state = command_ok(&state, undo, false);
    assert_eq!(doc(&state), "");
}

#[test]
fn can_go_back_and_forth_through_history_multiple_times() {
    let mut state = type_(&mk(), "one");
    state = type_(&state, " two");
    state = isolate(&state, IsolateHistory::Before);
    state = type_(&state, " three");
    state = type_at(&state, "zero ", 0);
    state = isolate(&state, IsolateHistory::Before);
    state = type_at(&state, "\n\n", 0);
    state = type_at(&state, "top", 0);
    for i in 0..6 {
        let re = i % 2 == 1;
        for _ in 0..4 {
            state = command(&state, if re { redo } else { undo });
        }
        assert_eq!(doc(&state), if re { "top\n\nzero one two three" } else { "" });
    }
}

#[test]
fn supports_non_tracked_changes_next_to_tracked_changes() {
    let mut state = type_(&mk(), "o");
    state = type_at(&state, "\n\n", 0);
    state = receive(&state, "zzz", 3, 3);
    state = command(&state, undo);
    assert_eq!(doc(&state), "zzz");
}

#[test]
fn can_go_back_and_forth_through_history_when_preserving_items() {
    let mut state = type_(&mk(), "one");
    state = type_(&state, " two");
    state = isolate(&state, IsolateHistory::Before);
    let len = state.doc().len();
    state = receive(&state, "xxx", len, len);
    state = type_(&state, " three");
    state = type_at(&state, "zero ", 0);
    state = isolate(&state, IsolateHistory::Before);
    state = type_at(&state, "\n\n", 0);
    state = type_at(&state, "top", 0);
    state = receive(&state, "yyy", 0, 0);
    for _ in 0..3 {
        for _ in 0..4 {
            state = command(&state, undo);
        }
        assert_eq!(doc(&state), "yyyxxx");
        for _ in 0..4 {
            state = command(&state, redo);
        }
        assert_eq!(doc(&state), "yyytop\n\nzero one twoxxx three");
    }
}

#[test]
fn restores_selection_on_undo() {
    let mut state = type_(&mk(), "hi");
    state = isolate(&state, IsolateHistory::Before);
    state = select(&state, 0, 2);
    let selection = state.selection().clone();
    state =
        update(&state, TransactionSpec::new().changes(vec![ChangeSpec::delete(0, 2), ChangeSpec::insert(0, "hello")]));
    let selection2 = state.selection().clone();
    state = command(&state, undo);
    assert!(state.selection().eq(&selection, false));
    state = command(&state, redo);
    assert!(state.selection().eq(&selection2, false));
}

#[test]
fn restores_the_selection_before_the_first_change_in_an_item() {
    let mut state = mk();
    state = update(
        &state,
        TransactionSpec::new().changes(ChangeSpec::insert(0, "a")).selection(EditorSelection::single(1, 1)),
    );
    state = update(
        &state,
        TransactionSpec::new().changes(ChangeSpec::insert(1, "b")).selection(EditorSelection::single(2, 2)),
    );
    state = command(&state, undo);
    assert_eq!(doc(&state), "");
    assert_eq!(state.selection().main().anchor(), 0);
}

#[test]
fn doesnt_merge_document_changes_if_theres_a_selection_change_in_between() {
    let mut state = type_(&mk(), "hi");
    state = select(&state, 0, 2);
    state = update(&state, state.replace_selection("hello"));
    assert_eq!(undo_depth(&state), 2);
}

#[test]
fn rebases_selection_on_undo() {
    let mut state = type_(&mk(), "hi");
    state = isolate(&state, IsolateHistory::Before);
    state = select(&state, 0, 2);
    state = type_at(&state, "hello", 0);
    state = receive(&state, "---", 0, 0);
    state = command(&state, undo);
    assert_eq!(state.selection().ranges[0].head(), 5);
}

#[test]
fn supports_querying_for_the_undo_and_redo_depth() {
    let mut state = type_(&mk(), "a");
    assert_eq!((undo_depth(&state), redo_depth(&state)), (1, 0));
    state = receive(&state, "b", 0, 0);
    assert_eq!((undo_depth(&state), redo_depth(&state)), (1, 0));
    state = command(&state, undo);
    assert_eq!((undo_depth(&state), redo_depth(&state)), (0, 1));
    state = command(&state, redo);
    assert_eq!((undo_depth(&state), redo_depth(&state)), (1, 0));
}

#[test]
fn all_functions_gracefully_handle_editor_states_without_history() {
    let state = EditorState::default();
    assert_eq!(undo_depth(&state), 0);
    assert_eq!(redo_depth(&state), 0);
    command_ok(&state, undo, false);
    command_ok(&state, redo, false);
}

#[test]
fn truncates_history() {
    let mut state = mk_state(HistoryConfig { min_depth: Some(10), ..Default::default() }, "");
    for _ in 0..40 {
        state = type_(&state, "a");
        state = isolate(&state, IsolateHistory::Before);
    }
    assert!(undo_depth(&state) < 40);
}

#[test]
fn doesnt_undo_selection_only_transactions() {
    let mut state = mk_state(HistoryConfig::default(), "abc");
    assert_eq!(state.selection().main().head(), 0);
    state = select(&state, 2, 2);
    state = command_ok(&state, undo, false);
    assert_eq!(state.selection().main().head(), 2);
}

#[test]
fn isolates_transactions_when_asked_to() {
    let mut state = mk();
    let ins = |state: &EditorState, at: usize, text: &str, iso: Option<IsolateHistory>| {
        let mut spec = TransactionSpec::new().changes(ChangeSpec::insert(at, text));
        if let Some(iso) = iso {
            spec = spec.annotation(ISOLATE_HISTORY.of(iso));
        }
        update(state, spec)
    };
    state = ins(&state, 0, "a", Some(IsolateHistory::After));
    state = ins(&state, 1, "a", None);
    state = ins(&state, 2, "c", Some(IsolateHistory::After));
    state = ins(&state, 3, "d", None);
    state = ins(&state, 4, "e", Some(IsolateHistory::Full));
    state = ins(&state, 5, "f", None);
    assert_eq!(undo_depth(&state), 5);
}

#[test]
fn can_group_events_around_a_non_history_transaction() {
    let mut state = mk();
    state = update(&state, TransactionSpec::new().changes(ChangeSpec::insert(0, "a")));
    state =
        update(&state, TransactionSpec::new().changes(ChangeSpec::insert(1, "b")).annotation(ADD_TO_HISTORY.of(false)));
    state = update(&state, TransactionSpec::new().changes(ChangeSpec::insert(1, "c")));
    state = command(&state, undo);
    assert_eq!(doc(&state), "b");
}

#[test]
fn properly_maps_selections_through_non_history_changes() {
    let mut state = mk_state(HistoryConfig::default(), "abc");
    let cursors = (0..3).map(EditorSelection::cursor).collect();
    state = update(&state, TransactionSpec::new().selection(EditorSelection::create(cursors, 0)));
    state = update(&state, TransactionSpec::new().changes(ChangeSpec::replace(0, 3, "d")));
    state = update(
        &state,
        TransactionSpec::new()
            .changes(vec![ChangeSpec::insert(0, "x"), ChangeSpec::insert(1, "y")])
            .annotation(ADD_TO_HISTORY.of(false)),
    );
    state = command(&state, undo);
    assert_eq!(doc(&state), "xabcy");
    let froms: Vec<usize> = state.selection().ranges.iter().map(|r| r.from).collect();
    assert_eq!(froms, [0, 2, 3]);
}

#[test]
fn restores_selection_on_redo() {
    let mut state = mk_state(HistoryConfig::default(), "a\nb\nc\n");
    let cursors = [1, 3, 5].into_iter().map(EditorSelection::cursor).collect();
    state = update(&state, TransactionSpec::new().selection(EditorSelection::create(cursors, 0)));
    state = update(&state, state.replace_selection("-"));
    state = select(&state, 0, 0);
    state = command(&state, undo);
    state = select(&state, 0, 0);
    state = command(&state, redo);
    let heads: Vec<usize> = state.selection().ranges.iter().map(|r| r.head()).collect();
    assert_eq!(heads, [2, 5, 8]);
}

// undoSelection

fn select_event(state: &EditorState, anchor: usize, head: usize) -> EditorState {
    update(state, TransactionSpec::new().selection(EditorSelection::single(anchor, head)).user_event("select"))
}

#[test]
fn undo_selection_allows_to_undo_a_change() {
    let mut state = type_(&mk(), "newtext");
    state = command(&state, undo_selection);
    assert_eq!(doc(&state), "");
}

#[test]
fn undo_selection_allows_to_undo_selection_only_transactions() {
    let mut state = mk_state(HistoryConfig::default(), "abc");
    assert_eq!(state.selection().main().head(), 0);
    state = select(&state, 2, 2);
    state = command(&state, undo_selection);
    assert_eq!(state.selection().main().head(), 0);
}

#[test]
fn undo_selection_merges_selection_only_transactions_from_keyboard() {
    let mut state = mk_state(HistoryConfig::default(), "abc");
    state = select_event(&state, 2, 2);
    state = select_event(&state, 3, 3);
    state = select_event(&state, 1, 1);
    state = command(&state, undo_selection);
    assert_eq!(state.selection().main().head(), 0);
}

#[test]
fn undo_selection_doesnt_merge_selection_only_transactions_from_other_sources() {
    let mut state = mk_state(HistoryConfig::default(), "abc");
    state = select(&state, 2, 2);
    state = select(&state, 3, 3);
    state = select(&state, 1, 1);
    state = command(&state, undo_selection);
    assert_eq!(state.selection().main().head(), 3);
    state = command(&state, undo_selection);
    assert_eq!(state.selection().main().head(), 2);
    state = command(&state, undo_selection);
    assert_eq!(state.selection().main().head(), 0);
}

#[test]
fn undo_selection_doesnt_merge_if_they_change_the_number_of_selections() {
    let mut state = mk_state(HistoryConfig::default(), "abc");
    state = select_event(&state, 2, 2);
    let two = EditorSelection::create(vec![EditorSelection::cursor(1), EditorSelection::cursor(3)], 0);
    state = update(&state, TransactionSpec::new().selection(two).user_event("select"));
    state = select_event(&state, 1, 1);
    state = command(&state, undo_selection);
    assert_eq!(state.selection().ranges.len(), 2);
    state = command(&state, undo_selection);
    assert_eq!(state.selection().main().head(), 0);
}

#[test]
fn undo_selection_doesnt_merge_if_a_selection_changes_empty_state() {
    let mut state = mk_state(HistoryConfig::default(), "abc");
    state = select_event(&state, 2, 2);
    state = select_event(&state, 2, 3);
    state = select_event(&state, 1, 1);
    state = command(&state, undo_selection);
    assert_eq!((state.selection().main().anchor(), state.selection().main().head()), (2, 3));
    state = command(&state, undo_selection);
    assert_eq!(state.selection().main().head(), 0);
}

#[test]
fn undo_selection_allows_to_redo_a_change() {
    let mut state = type_(&mk(), "newtext");
    state = command(&command(&state, undo_selection), redo_selection);
    assert_eq!(doc(&state), "newtext");
}

#[test]
fn undo_selection_allows_to_redo_selection_only_transactions() {
    let mut state = mk_state(HistoryConfig::default(), "abc");
    state = select(&state, 2, 2);
    state = command(&command(&state, undo_selection), redo_selection);
    assert_eq!(state.selection().main().head(), 2);
}

#[test]
fn undo_selection_only_changes_selection() {
    let mut state = type_(&mk(), "hi");
    state = isolate(&state, IsolateHistory::Before);
    let selection = state.selection().clone();
    state = select(&state, 0, 2);
    let selection2 = state.selection().clone();
    state = command(&state, undo_selection);
    assert!(state.selection().eq(&selection, false));
    assert_eq!(doc(&state), "hi");
    state = command(&state, redo_selection);
    assert!(state.selection().eq(&selection2, false));
    state = update(&state, state.replace_selection("hello"));
    state = command(&state, undo_selection);
    assert!(state.selection().eq(&selection2, false));
    state = command(&state, redo);
    assert_eq!(state.selection().main().head(), 5);
}

#[test]
fn undo_selection_can_undo_a_selection_through_remote_changes() {
    let mut state = type_(&mk(), "hello");
    let selection = state.selection().clone();
    state = select(&state, 0, 2);
    state = receive(&state, "oops", 0, 0);
    state = receive(&state, "!", 9, 9);
    assert!(state.selection().eq(&EditorSelection::single(4, 6), false));
    state = command(&state, undo_selection);
    assert_eq!(doc(&state), "oopshello!");
    assert!(state.selection().eq(&selection, false));
}

#[test]
fn undo_selection_preserves_text_inserted_inside_a_change() {
    let mut state = type_(&mk(), "1234");
    state =
        update(&state, TransactionSpec::new().changes(ChangeSpec::insert(2, "x")).annotation(ADD_TO_HISTORY.of(false)));
    state = command(&state, undo);
    assert_eq!(doc(&state), "x");
}

// effects

#[test]
fn includes_inverted_effects_in_the_history() {
    let set: StateEffectType<i64> = StateEffectType::define();
    let s = set.clone();
    let field = StateField::define(StateFieldSpec::new(
        |_| 0i64,
        move |val: &i64, tr: &Transaction| tr.effects().iter().fold(*val, |v, e| e.value(&s).copied().unwrap_or(v)),
    ));
    let (s, f) = (set.clone(), field.clone());
    let invert = INVERTED_EFFECTS.of(Arc::new(move |tr: &Transaction| {
        if tr.effects().iter().any(|e| e.is(&s)) {
            vec![s.of(*tr.start_state().field(&f))]
        } else {
            vec![]
        }
    }));
    let mut state = EditorState::create(EditorStateConfig::new().extensions(vec![
        history(HistoryConfig::default()),
        (&field).into(),
        invert,
    ]));
    for v in [10, 20] {
        state = update(
            &state,
            TransactionSpec::new().effect(set.of(v)).annotation(ISOLATE_HISTORY.of(IsolateHistory::Before)),
        );
    }
    assert_eq!(*state.field(&field), 20);
    for (cmd, expected) in [(undo as StateCommand, 10), (undo, 0), (redo, 10), (redo, 20), (undo, 10), (redo, 20)] {
        state = command(&state, cmd);
        assert_eq!(*state.field(&field), expected);
    }
}

#[derive(Clone, PartialEq, Debug)]
struct Comment {
    from: usize,
    to: usize,
    text: String,
}

fn map_comment(comment: &Comment, mapping: &ChangeDesc) -> Option<Comment> {
    let from = mapping.map_pos(comment.from, 1);
    let to = mapping.map_pos(comment.to, -1);
    if from >= to {
        None
    } else {
        Some(Comment { from, to, text: comment.text.clone() })
    }
}

struct CommentSetup {
    add: StateEffectType<Comment>,
    comments: StateField<Vec<Comment>>,
    extension: Extension,
}

fn comment_setup() -> CommentSetup {
    let add: StateEffectType<Comment> = StateEffectType::define_with_map(map_comment);
    let rm: StateEffectType<Comment> = StateEffectType::define_with_map(map_comment);
    let (a, r) = (add.clone(), rm.clone());
    let comments = StateField::define(StateFieldSpec::new(
        |_| Vec::<Comment>::new(),
        move |value: &Vec<Comment>, tr: &Transaction| {
            let mut value: Vec<Comment> = value.iter().filter_map(|c| map_comment(c, tr.changes().desc())).collect();
            for effect in tr.effects() {
                if let Some(c) = effect.value(&a) {
                    value.push(c.clone());
                } else if let Some(c) = effect.value(&r) {
                    value.retain(|x| x != c);
                }
            }
            value.sort_by_key(|c| c.from);
            value
        },
    ));
    let (a, r, f) = (add.clone(), rm.clone(), comments.clone());
    let invert = INVERTED_EFFECTS.of(Arc::new(move |tr: &Transaction| {
        let mut effects = Vec::new();
        for effect in tr.effects() {
            let (value, is_add) = match (effect.value(&a), effect.value(&r)) {
                (Some(c), _) => (c, true),
                (_, Some(c)) => (c, false),
                _ => continue,
            };
            if let Some(src) = map_comment(value, &tr.changes().inverted_desc()) {
                effects.push(if is_add { r.of(src) } else { a.of(src) });
            }
        }
        for comment in tr.start_state().field(&f) {
            if map_comment(comment, tr.changes().desc()).is_none() {
                effects.push(a.of(comment.clone()));
            }
        }
        effects
    }));
    let extension = Extension::list([history(HistoryConfig::default()), (&comments).into(), invert]);
    CommentSetup { add, comments, extension }
}

fn comment_str(state: &EditorState, comments: &StateField<Vec<Comment>>) -> String {
    state.field(comments).iter().map(|c| format!("{}@{}", c.text, c.from)).collect::<Vec<_>>().join(",")
}

fn comment(from: usize, to: usize, text: &str) -> Comment {
    Comment { from, to, text: text.to_string() }
}

#[test]
fn can_map_effects() {
    let CommentSetup { add, comments, extension } = comment_setup();
    let full = || ISOLATE_HISTORY.of(IsolateHistory::Full);
    let mut state = EditorState::create(EditorStateConfig::new().doc("one two foo").extensions(extension));
    state = update(&state, TransactionSpec::new().effect(add.of(comment(0, 3, "c1"))).annotation(full()));
    assert_eq!(comment_str(&state, &comments), "c1@0");
    state = update(
        &state,
        TransactionSpec::new()
            .changes(ChangeSpec::replace(3, 4, "---"))
            .annotation(full())
            .effect(add.of(comment(6, 9, "c2"))),
    );
    assert_eq!(comment_str(&state, &comments), "c1@0,c2@6");
    state = update(
        &state,
        TransactionSpec::new().changes(ChangeSpec::insert(0, "---")).annotation(ADD_TO_HISTORY.of(false)),
    );
    assert_eq!(comment_str(&state, &comments), "c1@3,c2@9");
    state = command(&state, undo);
    assert_eq!(doc(&state), "---one two foo");
    assert_eq!(comment_str(&state, &comments), "c1@3");
    state = command(&state, undo);
    assert_eq!(comment_str(&state, &comments), "");
    state = command(&state, redo);
    assert_eq!(comment_str(&state, &comments), "c1@3");
    state = command(&state, redo);
    assert_eq!(comment_str(&state, &comments), "c1@3,c2@9");
    assert_eq!(doc(&state), "---one---two foo");
    state = command(&state, undo);
    state = update(
        &state,
        TransactionSpec::new().changes(ChangeSpec::replace(10, 11, "---")).annotation(ADD_TO_HISTORY.of(false)),
    );
    state = update(&state, TransactionSpec::new().effect(add.of(comment(13, 16, "c3"))).annotation(full()));
    assert_eq!(comment_str(&state, &comments), "c1@3,c3@13");
    state = command(&state, undo);
    assert_eq!(doc(&state), "---one two---foo");
    assert_eq!(comment_str(&state, &comments), "c1@3");
    state = command(&state, redo);
    assert_eq!(comment_str(&state, &comments), "c1@3,c3@13");
}

#[test]
fn can_restore_comments_lost_through_deletion() {
    let CommentSetup { add, comments, extension } = comment_setup();
    let mut state = EditorState::create(EditorStateConfig::new().doc("123456").extensions(extension));
    state = update(
        &state,
        TransactionSpec::new().effect(add.of(comment(3, 5, "c1"))).annotation(ISOLATE_HISTORY.of(IsolateHistory::Full)),
    );
    state = update(&state, TransactionSpec::new().changes(ChangeSpec::delete(2, 6)));
    assert_eq!(comment_str(&state, &comments), "");
    state = command(&state, undo);
    assert_eq!(comment_str(&state, &comments), "c1@3");
}

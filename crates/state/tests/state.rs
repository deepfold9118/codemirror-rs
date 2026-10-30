//! Port of CodeMirror's `test/test-state.ts`. JSON serialization is not
//! ported yet, so its test is left out.

use std::sync::{Arc, LazyLock};

use codemirror_state::{
    AnnotationType, ChangeFilterResult, ChangeSpec, Compartment, Dep, EditorSelection, EditorState, EditorStateConfig,
    Extension, Facet, FacetSpec, RangeChange, StateEffect, StateEffectType, StateField, StateFieldSpec,
    TransactionFilterResult, TransactionSpec, ALLOW_MULTIPLE_SELECTIONS, CHANGE_FILTER, LINE_SEPARATOR, TAB_SIZE,
    TRANSACTION_EXTENDER, TRANSACTION_FILTER,
};

fn create(doc: &str, extensions: Vec<Extension>) -> EditorState {
    EditorState::create(EditorStateConfig::new().doc(doc).extensions(extensions))
}

fn join<T: ToString>(values: &[T]) -> String {
    values.iter().map(ToString::to_string).collect::<Vec<_>>().join(",")
}

fn panics(f: impl FnOnce()) -> String {
    let err = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).expect_err("should panic");
    err.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| err.downcast_ref::<String>().cloned())
        .unwrap_or_default()
}

#[test]
fn holds_doc_and_selection_properties() {
    let state = create("hello", vec![]);
    assert_eq!(state.doc().to_string(), "hello");
    assert_eq!(state.selection().main().from, 0);
}

#[test]
fn can_apply_changes() {
    let state = create("hello", vec![]);
    let tr =
        state.update(TransactionSpec::new().changes(vec![ChangeSpec::replace(2, 4, "w"), ChangeSpec::insert(5, "!")]));
    assert_eq!(tr.state().doc().to_string(), "hewo!");
}

#[test]
fn maps_selection_through_changes() {
    let state = EditorState::create(
        EditorStateConfig::new()
            .doc("abcdefgh")
            .extensions(vec![ALLOW_MULTIPLE_SELECTIONS.of(true)])
            .selection(EditorSelection::create([0, 4, 8].map(EditorSelection::cursor).to_vec(), 0)),
    );
    let new_state = state.update(state.replace_selection("Q")).state().clone();
    assert_eq!(new_state.doc().to_string(), "QabcdQefghQ");
    let froms: Vec<usize> = new_state.selection().ranges.iter().map(|r| r.from).collect();
    assert_eq!(froms, vec![1, 6, 11]);
}

static SOME_ANNOTATION: LazyLock<AnnotationType<i64>> = LazyLock::new(AnnotationType::define);

#[test]
fn can_store_annotations_on_transactions() {
    let tr = create("foo", vec![]).update(TransactionSpec::new().annotation(SOME_ANNOTATION.of(55)));
    assert_eq!(tr.annotation(&SOME_ANNOTATION), Some(&55));
}

#[test]
fn throws_when_a_changes_bounds_are_invalid() {
    let state = create("1234", vec![]);
    let s = state.clone();
    panics(move || {
        s.update(TransactionSpec::new().changes(ChangeSpec::delete(2, 1)));
    });
    let s = state.clone();
    panics(move || {
        s.update(TransactionSpec::new().changes(ChangeSpec::replace(2, 10, "x")));
    });
}

#[test]
fn stores_and_updates_tab_size() {
    let deflt = EditorState::default();
    let two = create("", vec![TAB_SIZE.of(2)]);
    assert_eq!(deflt.tab_size(), 4);
    assert_eq!(two.tab_size(), 2);
    let updated = deflt.update(TransactionSpec::new().effect(StateEffect::reconfigure(TAB_SIZE.of(8))));
    assert_eq!(updated.state().tab_size(), 8);
}

#[test]
fn stores_and_updates_the_line_separator() {
    let deflt = EditorState::default();
    let crlf = create("", vec![LINE_SEPARATOR.of("\r\n".to_string())]);
    assert_eq!(*deflt.facet(&LINE_SEPARATOR), None);
    assert_eq!(deflt.to_text("a\nb").lines(), 2);
    assert_eq!(crlf.facet(&LINE_SEPARATOR).as_deref(), Some("\r\n"));
    // CodeMirror keeps a lone "\n" inside a line when the separator is
    // "\r\n"; this port always treats "\n" as a line break.
    assert_eq!(crlf.to_text("a\r\nb").lines(), 2);
    assert_eq!(crlf.to_text("a\rb").lines(), 1);
    let updated = crlf.update(TransactionSpec::new().effect(StateEffect::reconfigure(LINE_SEPARATOR.of("\n".into()))));
    assert_eq!(updated.state().facet(&LINE_SEPARATOR).as_deref(), Some("\n"));
}

#[test]
fn stores_and_updates_fields() {
    let field1 = StateField::define(StateFieldSpec::new(|_| 0i64, |v: &i64, _| v + 1));
    let f1 = field1.clone();
    let field2 = StateField::define(StateFieldSpec::new(move |s: &EditorState| s.field(&f1) + 10, |v: &i64, _| *v));
    let state = create("", vec![(&field1).into(), (&field2).into()]);
    assert_eq!(*state.field(&field1), 0);
    assert_eq!(*state.field(&field2), 10);
    let new_state = state.update(TransactionSpec::new()).state().clone();
    assert_eq!(*new_state.field(&field1), 1);
    assert_eq!(*new_state.field(&field2), 10);
}

#[test]
fn allows_fields_to_have_an_initializer() {
    let field = StateField::define(StateFieldSpec::new(|_| 0i64, |v: &i64, _| v + 1));
    let state = create("", vec![field.init(|_| 10)]);
    assert_eq!(*state.field(&field), 10);
    assert_eq!(*state.update(TransactionSpec::new()).state().field(&field), 11);
}

#[test]
fn can_preserve_fields_across_reconfiguration() {
    let field = StateField::define(StateFieldSpec::new(|_| 0i64, |v: &i64, _| v + 1));
    let start = create("", vec![(&field).into()]).update(TransactionSpec::new()).state().clone();
    assert_eq!(*start.field(&field), 1);
    let kept = start.update(TransactionSpec::new().effect(StateEffect::reconfigure(&field)));
    assert_eq!(*kept.state().field(&field), 2);
    let dropped = start.update(TransactionSpec::new().effect(StateEffect::reconfigure(Extension::empty())));
    assert_eq!(dropped.state().field_opt(&field), None);
}

#[test]
fn can_replace_extension_groups() {
    let comp = Compartment::new();
    let f: Facet<i64> = Facet::list();
    let content = f.of(10);
    let state = create("", vec![comp.of(&content), f.of(20)]);
    assert!(comp.get(&state).unwrap().ptr_eq(&content));
    assert_eq!(join(state.facet(&f)), "10,20");
    let content2 = Extension::list([f.of(1), f.of(2)]);
    let state2 = state.update(TransactionSpec::new().effect(comp.reconfigure(&content2))).state().clone();
    assert!(comp.get(&state2).unwrap().ptr_eq(&content2));
    assert_eq!(join(state2.facet(&f)), "1,2,20");
    let state3 = state2.update(TransactionSpec::new().effect(comp.reconfigure(f.of(3)))).state().clone();
    assert_eq!(join(state3.facet(&f)), "3,20");
}

#[test]
fn raises_an_error_on_duplicate_extension_groups() {
    let comp = Compartment::new();
    let f: Facet<i64> = Facet::list();
    let (c, g) = (comp.clone(), f.clone());
    let msg = panics(move || {
        create("", vec![c.of(g.of(1)), c.of(g.of(2))]);
    });
    assert!(msg.to_lowercase().contains("duplicate use of compartment"));
    let msg = panics(move || {
        create("", vec![comp.of(comp.of(f.of(1)))]);
    });
    assert!(msg.to_lowercase().contains("duplicate use of compartment"));
}

#[test]
fn preserves_compartments_on_reconfigure() {
    let comp = Compartment::new();
    let f: Facet<i64> = Facet::list();
    let init = comp.of(f.of(10));
    let mut state = create("", vec![init.clone(), f.of(20)]);
    state = state.update(TransactionSpec::new().effect(comp.reconfigure(f.of(0)))).state().clone();
    assert_eq!(join(state.facet(&f)), "0,20");
    state = state.update(TransactionSpec::new().effect(StateEffect::reconfigure(vec![init, f.of(2)]))).state().clone();
    assert_eq!(join(state.facet(&f)), "0,2");
}

#[test]
fn forgets_dropped_compartments() {
    let comp = Compartment::new();
    let f: Facet<i64> = Facet::list();
    let init = comp.of(f.of(10));
    let mut state = create("", vec![init.clone(), f.of(20)]);
    state = state.update(TransactionSpec::new().effect(comp.reconfigure(f.of(0)))).state().clone();
    assert_eq!(join(state.facet(&f)), "0,20");
    state = state.update(TransactionSpec::new().effect(StateEffect::reconfigure(f.of(2)))).state().clone();
    assert_eq!(join(state.facet(&f)), "2");
    assert!(comp.get(&state).is_none());
    state = state.update(TransactionSpec::new().effect(StateEffect::reconfigure(vec![init, f.of(2)]))).state().clone();
    assert_eq!(join(state.facet(&f)), "10,2");
}

#[test]
fn allows_facets_computed_from_fields() {
    let field = StateField::define(
        StateFieldSpec::new(
            |_| vec![0usize],
            |v: &Vec<usize>, tr| {
                if tr.doc_changed() {
                    vec![tr.state().doc().len()]
                } else {
                    v.clone()
                }
            },
        )
        .compare_eq(),
    );
    let facet: Facet<usize> = Facet::define(FacetSpec::list().compare_input_eq());
    let fl = field.clone();
    let state =
        create("", vec![(&field).into(), facet.compute(vec![field.dep()], move |s| s.field(&fl)[0]), facet.of(1)]);
    assert_eq!(join(state.facet(&facet)), "0,1");
    let state2 = state.update(TransactionSpec::new()).state().clone();
    assert!(std::ptr::eq(state2.facet(&facet), state.facet(&facet)));
    let state3 = state.update(TransactionSpec::new().changes(ChangeSpec::insert(0, "hi"))).state().clone();
    assert_eq!(join(state3.facet(&facet)), "2,1");
}

#[test]
fn blocks_multiple_selections_when_not_allowed() {
    let cursors = EditorSelection::create(vec![EditorSelection::cursor(0), EditorSelection::cursor(1)], 0);
    let state = EditorState::create(EditorStateConfig::new().doc("123").selection(cursors.clone()));
    assert_eq!(state.selection().ranges.len(), 1);
    assert_eq!(state.update(TransactionSpec::new().selection(cursors)).state().selection().ranges.len(), 1);
}

#[test]
fn change_by_range_can_make_simple_changes() {
    let mut state = create("hi", vec![]);
    let spec = state.change_by_range(|r| {
        RangeChange::new(EditorSelection::cursor(r.from + 1)).changes(ChangeSpec::replace(r.from, r.from + 1, "q"))
    });
    state = state.update(spec).state().clone();
    assert_eq!(state.doc().to_string(), "qi");
    assert_eq!(state.selection().main().from, 1);
}

#[test]
fn change_by_range_does_the_right_thing_with_multiple_selections() {
    let mut state = EditorState::create(
        EditorStateConfig::new()
            .doc("1 2 3 4")
            .selection(EditorSelection::create(
                vec![
                    EditorSelection::range(0, 1),
                    EditorSelection::range(2, 3),
                    EditorSelection::range(4, 5),
                    EditorSelection::range(6, 7),
                ],
                0,
            ))
            .extensions(ALLOW_MULTIPLE_SELECTIONS.of(true)),
    );
    let spec = state.change_by_range(|r| {
        RangeChange::new(EditorSelection::range(r.from, r.from + 1 + (r.from >> 1))).changes(ChangeSpec::replace(
            r.from,
            r.to,
            "-".repeat((r.from >> 1) + 1),
        ))
    });
    state = state.update(spec).state().clone();
    assert_eq!(state.doc().to_string(), "- -- --- ----");
    let ranges: Vec<String> = state.selection().ranges.iter().map(|r| format!("{}-{}", r.from, r.to)).collect();
    assert_eq!(ranges.join(" "), "0-1 2-4 5-8 9-13");
}

#[test]
fn change_filter_can_cancel_changes() {
    // Cancels all changes that add length
    let state = create(
        "one two",
        vec![CHANGE_FILTER.of(Arc::new(|tr| {
            if tr.changes().new_length() <= tr.changes().length() {
                ChangeFilterResult::Allow
            } else {
                ChangeFilterResult::Block
            }
        }))],
    );
    let tr1 = state.update(
        TransactionSpec::new().changes(ChangeSpec::insert(3, " three")).selection(EditorSelection::single(13, 13)),
    );
    assert_eq!(tr1.state().doc().to_string(), "one two");
    assert_eq!(tr1.state().selection().main().head(), 7);
    let tr2 = state.update(TransactionSpec::new().changes(ChangeSpec::replace(4, 7, "2")));
    assert_eq!(tr2.state().doc().to_string(), "one 2");
}

#[test]
fn change_filter_can_split_changes() {
    // Disallows changes in the middle third of the document
    let state = create(
        "onetwo",
        vec![CHANGE_FILTER.of(Arc::new(|tr| {
            let len = tr.start_state().doc().len();
            ChangeFilterResult::Ranges(vec![len / 3, 2 * len / 3])
        }))],
    );
    assert_eq!(state.update(TransactionSpec::new().changes(ChangeSpec::delete(0, 6))).state().doc().to_string(), "et");
}

#[test]
fn change_filter_combines_filter_masks() {
    let state = create(
        "onetwo",
        vec![
            CHANGE_FILTER.of(Arc::new(|_| ChangeFilterResult::Ranges(vec![0, 2]))),
            CHANGE_FILTER.of(Arc::new(|_| ChangeFilterResult::Ranges(vec![4, 6]))),
        ],
    );
    assert_eq!(
        state.update(TransactionSpec::new().changes(ChangeSpec::delete(0, 6))).state().doc().to_string(),
        "onwo"
    );
}

#[test]
fn change_filter_can_be_turned_off() {
    let state = create("", vec![CHANGE_FILTER.of(Arc::new(|_| ChangeFilterResult::Block))]);
    assert_eq!(state.update(TransactionSpec::new().changes(ChangeSpec::insert(0, "hi"))).state().doc().len(), 0);
    let tr = state.update(TransactionSpec::new().changes(ChangeSpec::insert(0, "hi")).no_filter());
    assert_eq!(tr.state().doc().len(), 2);
}

#[test]
fn transaction_filter_can_constrain_the_selection() {
    let state = create(
        "one two",
        vec![TRANSACTION_FILTER.of(Arc::new(|tr| match tr.selection() {
            Some(sel) if sel.main().to > 4 => TransactionFilterResult::Specs(vec![
                tr.into(),
                TransactionSpec::new().selection(EditorSelection::single(4, 4)),
            ]),
            _ => TransactionFilterResult::Keep,
        }))],
    );
    let sel = |anchor| state.update(TransactionSpec::new().selection(EditorSelection::single(anchor, anchor)));
    assert_eq!(sel(3).selection().unwrap().main().to, 3);
    assert_eq!(sel(7).selection().unwrap().main().to, 4);
}

#[test]
fn transaction_filter_can_append_sequential_changes() {
    let state = create(
        "one two",
        vec![TRANSACTION_FILTER.of(Arc::new(|tr| {
            TransactionFilterResult::Specs(vec![
                tr.into(),
                TransactionSpec::new().changes(ChangeSpec::insert(tr.changes().new_length(), "!")).sequential(),
            ])
        }))],
    );
    let tr = state.update(TransactionSpec::new().changes(ChangeSpec::insert(3, ",")));
    assert_eq!(tr.state().doc().to_string(), "one, two!");
}

#[test]
fn transaction_extender_can_add_annotations() {
    let ann: AnnotationType<i64> = AnnotationType::define();
    let a = ann;
    let state = create(
        "",
        vec![TRANSACTION_EXTENDER.of(Arc::new(move |_| Some(TransactionSpec::new().annotation(a.of(100)))))],
    );
    let tr = state.update(TransactionSpec::new().changes(ChangeSpec::insert(0, "!")));
    assert_eq!(tr.annotation(&ann), Some(&100));
    let tr_no_filter = state.update(TransactionSpec::new().changes(ChangeSpec::insert(0, "!")).no_filter());
    assert_eq!(tr_no_filter.annotation(&ann), Some(&100));
}

#[test]
fn allows_multiple_extenders_to_take_effect() {
    let eff: StateEffectType<i64> = StateEffectType::define();
    let (e1, e2) = (eff.clone(), eff.clone());
    let state = create(
        "",
        vec![
            TRANSACTION_EXTENDER.of(Arc::new(move |_| Some(TransactionSpec::new().effect(e1.of(1))))),
            TRANSACTION_EXTENDER.of(Arc::new(move |_| Some(TransactionSpec::new().effect(e2.of(2))))),
        ],
    );
    let tr = state.update(TransactionSpec::new().scroll_into_view());
    let values: Vec<i64> = tr.effects().iter().map(|e| e.value(&eff).copied().unwrap_or(0)).collect();
    assert_eq!(values, vec![2, 1]);
}

#[test]
fn reports_user_events_and_word_ranges() {
    // Not in CodeMirror's suite: user-event matching and word lookup.
    let state = create("héllo wörld", vec![]);
    let tr = state.update(TransactionSpec::new().user_event("select.pointer"));
    assert!(tr.is_user_event("select"));
    assert!(tr.is_user_event("select.pointer"));
    assert!(!tr.is_user_event("sel"));
    let word = state.word_at(8).unwrap();
    assert_eq!((word.from, word.to), (7, 13));
    assert_eq!(state.slice_doc(word.from, word.to), "wörld");
    assert_eq!(state.phrase("Go to $1 of $2, $$5", &["line", "10"]), "Go to line of 10, $5");
    let _ = Dep::doc();
}

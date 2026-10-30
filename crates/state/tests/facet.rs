//! Port of CodeMirror's `test/test-facet.ts`.
//!
//! JavaScript compares numbers and strings by value with `===`; the test
//! facets here use `compare_input_eq` to get the same behavior. Object
//! identity checks become `std::ptr::eq` on the returned references.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use codemirror_state::{
    Dep, EditorState, EditorStateConfig, Extension, Facet, FacetSpec, Prec, StateEffect, StateEffectType, StateField,
    StateFieldSpec, TransactionSpec,
};

static NUM: LazyLock<Facet<i64>> = LazyLock::new(|| Facet::define(FacetSpec::list().compare_input_eq()));
static STR: LazyLock<Facet<String>> = LazyLock::new(|| Facet::define(FacetSpec::list().compare_input_eq()));
static BOOL: LazyLock<Facet<bool>> = LazyLock::new(|| Facet::define(FacetSpec::list().compare_input_eq()));

fn mk(extensions: Vec<Extension>) -> EditorState {
    EditorState::create(EditorStateConfig::new().extensions(extensions))
}

fn join<T: ToString>(values: &[T]) -> String {
    values.iter().map(ToString::to_string).collect::<Vec<_>>().join(",")
}

fn s(v: &str) -> Extension {
    STR.of(v.to_string())
}

fn empty_update(state: &EditorState) -> EditorState {
    state.update(TransactionSpec::new()).state().clone()
}

#[test]
fn allows_querying_of_facets() {
    let st = mk(vec![NUM.of(10), NUM.of(20), s("x"), s("y")]);
    assert_eq!(join(st.facet(&NUM)), "10,20");
    assert_eq!(join(st.facet(&STR)), "x,y");
}

#[test]
fn includes_sub_extenders() {
    let e = |s: &str| Extension::list([NUM.of(s.len() as i64), NUM.of(s.parse().unwrap())]);
    let st = mk(vec![NUM.of(5), e("20"), NUM.of(40), e("100")]);
    assert_eq!(join(st.facet(&NUM)), "5,2,20,40,3,100");
}

#[test]
fn only_includes_duplicated_extensions_once() {
    let e = NUM.of(50);
    let st = mk(vec![NUM.of(1), e.clone(), NUM.of(4), e]);
    assert_eq!(join(st.facet(&NUM)), "1,50,4");
}

#[test]
fn returns_an_empty_array_for_absent_facet() {
    let st = mk(vec![]);
    assert!(st.facet(&NUM).is_empty());
}

#[test]
fn sorts_extensions_by_priority() {
    let st = mk(vec![
        s("a"),
        s("b"),
        Prec::high(s("c")),
        Prec::highest(s("d")),
        Prec::low(s("e")),
        Prec::high(s("f")),
        s("g"),
    ]);
    assert_eq!(join(st.facet(&STR)), "d,c,f,a,b,g,e");
}

#[test]
fn lets_sub_extensions_inherit_their_parents_priority() {
    let e = |n: i64| NUM.of(n);
    let st = mk(vec![NUM.of(1), Prec::highest(e(2)), e(4)]);
    assert_eq!(join(st.facet(&NUM)), "2,1,4");
}

#[test]
fn supports_dynamic_facet() {
    let st = mk(vec![NUM.of(1), NUM.compute(vec![], |_| 88)]);
    assert_eq!(join(st.facet(&NUM)), "1,88");
}

#[test]
fn only_recomputes_a_facet_value_when_necessary() {
    let st = mk(vec![NUM.of(1), NUM.compute(vec![STR.dep()], |s| join(s.facet(&STR)).len() as i64), s("hello")]);
    let array = st.facet(&NUM);
    assert_eq!(join(array), "1,5");
    let next = empty_update(&st);
    assert!(std::ptr::eq(next.facet(&NUM), array));
}

#[test]
fn can_handle_dependencies_on_facets_that_arent_present_in_the_state() {
    let st = mk(vec![
        NUM.compute(vec![STR.dep()], |s| join(s.facet(&STR)).len() as i64),
        STR.compute(vec![BOOL.dep()], |s| join(s.facet(&BOOL))),
    ]);
    assert_eq!(join(empty_update(&st).facet(&NUM)), "0");
}

#[test]
fn can_specify_a_dependency_on_the_document() {
    let count = Arc::new(AtomicUsize::new(0));
    let c = count.clone();
    let mut st = mk(vec![NUM.compute(vec![Dep::doc()], move |_| c.fetch_add(1, Ordering::SeqCst) as i64)]);
    assert_eq!(join(st.facet(&NUM)), "0");
    st = st.update(TransactionSpec::new().changes(codemirror_state::ChangeSpec::insert(0, "hello"))).state().clone();
    assert_eq!(join(st.facet(&NUM)), "1");
    st = empty_update(&st);
    assert_eq!(join(st.facet(&NUM)), "1");
}

#[test]
fn can_specify_a_dependency_on_the_selection() {
    let count = Arc::new(AtomicUsize::new(0));
    let c = count.clone();
    let mut st = mk(vec![NUM.compute(vec![Dep::selection()], move |_| c.fetch_add(1, Ordering::SeqCst) as i64)]);
    assert_eq!(join(st.facet(&NUM)), "0");
    st = st.update(TransactionSpec::new().changes(codemirror_state::ChangeSpec::insert(0, "hello"))).state().clone();
    assert_eq!(join(st.facet(&NUM)), "1");
    st = st.update(TransactionSpec::new().selection(codemirror_state::EditorSelection::single(2, 2))).state().clone();
    assert_eq!(join(st.facet(&NUM)), "2");
    st = empty_update(&st);
    assert_eq!(join(st.facet(&NUM)), "2");
}

#[test]
fn can_provide_multiple_values_at_once() {
    let mut st = mk(vec![
        NUM.compute_n(vec![Dep::doc()], |s| if s.doc().len() % 2 == 1 { vec![100, 10] } else { vec![] }),
        NUM.of(1),
    ]);
    assert_eq!(join(st.facet(&NUM)), "1");
    st = st.update(TransactionSpec::new().changes(codemirror_state::ChangeSpec::insert(0, "hello"))).state().clone();
    assert_eq!(join(st.facet(&NUM)), "100,10,1");
}

#[test]
fn works_with_a_static_combined_facet() {
    let f: Facet<i64, i64> = Facet::define(FacetSpec::combine(|ns: &[i64]| ns.iter().sum()));
    let st = mk(vec![f.of(1), f.of(2), f.of(3)]);
    assert_eq!(*st.facet(&f), 6);
}

#[test]
fn works_with_a_dynamic_combined_facet() {
    let f: Facet<i64, i64> = Facet::define(FacetSpec::combine(|ns: &[i64]| ns.iter().sum()));
    let mut st = mk(vec![f.of(1), f.compute(vec![Dep::doc()], |s| s.doc().len() as i64), f.of(3)]);
    assert_eq!(*st.facet(&f), 4);
    st = st.update(TransactionSpec::new().changes(codemirror_state::ChangeSpec::insert(0, "hello"))).state().clone();
    assert_eq!(*st.facet(&f), 9);
}

#[test]
fn survives_reconfiguration() {
    let st = mk(vec![NUM.compute(vec![Dep::doc()], |s| s.doc().len() as i64), NUM.of(2), s("3")]);
    let st2 =
        st.update(TransactionSpec::new().effect(StateEffect::reconfigure(vec![
            NUM.compute(vec![Dep::doc()], |s| s.doc().len() as i64),
            NUM.of(2),
        ])))
        .state()
        .clone();
    assert!(std::ptr::eq(st.facet(&NUM), st2.facet(&NUM)));
    assert_eq!(st2.facet(&STR).len(), 0);
}

#[derive(Debug)]
struct Count {
    count: usize,
}

#[test]
fn survives_unrelated_reconfiguration_even_without_deep_compare() {
    let f: Facet<i64, Count> = Facet::define(FacetSpec::combine(|v: &[i64]| Count { count: v.len() }));
    let st = mk(vec![f.compute(vec![Dep::doc()], |s| s.doc().len() as i64), f.of(2)]);
    let st2 = st.update(TransactionSpec::new().effect(StateEffect::append_config(s("hi")))).state().clone();
    assert!(std::ptr::eq(st.facet(&f), st2.facet(&f)));
    assert_eq!(st2.facet(&f).count, 2);
}

#[test]
fn preserves_static_facets_across_reconfiguration() {
    let st = mk(vec![NUM.of(1), NUM.of(2), s("3")]);
    let st2 =
        st.update(TransactionSpec::new().effect(StateEffect::reconfigure(vec![NUM.of(1), NUM.of(2)]))).state().clone();
    assert!(std::ptr::eq(st.facet(&NUM), st2.facet(&NUM)));
}

#[test]
fn creates_newly_added_fields_when_reconfiguring() {
    let st = mk(vec![NUM.of(2)]);
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let (e1, e2) = (events.clone(), events.clone());
    let field = StateField::define(StateFieldSpec::new(
        move |_| {
            e1.lock().unwrap().push("create".into());
            0i64
        },
        move |val: &i64, _| {
            e2.lock().unwrap().push(format!("update {val}"));
            val + 1
        },
    ));
    let st = st.update(TransactionSpec::new().effect(StateEffect::append_config(&field))).state().clone();
    assert_eq!(events.lock().unwrap().join(", "), "create, update 0");
    assert_eq!(*st.field(&field), 1);
}

#[test]
fn applies_effects_from_reconfiguring_transaction_to_new_fields() {
    let st = mk(vec![]);
    let effect: StateEffectType<i64> = StateEffectType::define();
    let eff = effect.clone();
    let field = StateField::define(StateFieldSpec::new(
        |state: &EditorState| state.facet(&NUM).first().copied().unwrap_or(0),
        move |val: &i64, tr| tr.effects().iter().fold(*val, |v, e| v + e.value(&eff).copied().unwrap_or(0)),
    ));
    let st = st
        .update(
            TransactionSpec::new()
                .effect(StateEffect::append_config(vec![(&field).into(), NUM.of(10)]))
                .effect(effect.of(5)),
        )
        .state()
        .clone();
    assert_eq!(*st.field(&field), 15);
}

#[test]
fn errors_on_cyclic_dependencies() {
    let result = std::panic::catch_unwind(|| {
        mk(vec![
            NUM.compute(vec![STR.dep()], |s| s.facet(&STR).len() as i64),
            STR.compute(vec![NUM.dep()], |s| join(s.facet(&NUM))),
        ])
    });
    let err = result.err().expect("should panic");
    let msg = err.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| err.downcast_ref::<String>().cloned());
    assert!(msg.unwrap_or_default().to_lowercase().contains("cyclic"));
}

#[test]
fn updates_facets_computed_from_static_values_on_reconfigure() {
    let mut st = mk(vec![NUM.compute(vec![STR.dep()], |s| s.facet(&STR).len() as i64), s("A")]);
    st = st.update(TransactionSpec::new().effect(StateEffect::append_config(s("B")))).state().clone();
    assert_eq!(join(st.facet(&NUM)), "2");
    let st2 = st.update(TransactionSpec::new().effect(StateEffect::append_config(BOOL.of(false)))).state().clone();
    assert!(std::ptr::eq(st.facet(&NUM), st2.facet(&NUM)));
}

#[derive(Clone)]
struct A {
    #[allow(dead_code)]
    a: i64,
}

#[test]
fn preserves_dynamic_facet_values_when_dependencies_stay_the_same() {
    let f: Facet<A> = Facet::list();
    let st1 = mk(vec![f.compute(vec![], |_| A { a: 1 }), s("A")]);
    let st2 = st1.update(TransactionSpec::new().effect(StateEffect::append_config(BOOL.of(true)))).state().clone();
    assert!(std::ptr::eq(st1.facet(&f), st2.facet(&f)));
}

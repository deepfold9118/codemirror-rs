//! Port of CodeMirror's `test/test-change.ts`.

mod common;

use codemirror_state::{ChangeDesc, ChangeSet, ChangeSpec, MapMode, Text};
use common::Rng;

fn mk(spec: &str) -> ChangeDesc {
    let sections: Vec<(usize, Option<usize>)> = spec
        .split_whitespace()
        .map(|part| match part.split_once(':') {
            Some((len, ins)) => (len.parse().unwrap(), Some(ins.parse().unwrap())),
            None => (part.parse().unwrap(), None),
        })
        .collect();
    ChangeDesc::from_sections(&sections)
}

fn r_change(rng: &mut Rng, len: usize) -> ChangeSpec {
    if len == 0 || rng.below(3) == 0 {
        let n = rng.below(5) + 1;
        let insert = rng.string(n);
        return ChangeSpec::insert(rng.below(len), insert);
    }
    let from = rng.below(len - 1);
    let to = (from + rng.below(5) + 1).min(len);
    if rng.below(2) == 0 {
        let n = rng.below(2) + 1;
        ChangeSpec::replace(from, to, rng.string(n))
    } else {
        ChangeSpec::delete(from, to)
    }
}

fn r_changes(rng: &mut Rng, len: usize, count: usize) -> ChangeSpec {
    ChangeSpec::List((0..count).map(|_| r_change(rng, len)).collect())
}

fn of(spec: ChangeSpec, len: usize) -> ChangeSet {
    ChangeSet::of(spec, len, None)
}

// ---------- ChangeDesc composition ----------

fn comp(specs: &[&str]) {
    let (result, sets) = specs.split_last().unwrap();
    let composed = sets.iter().map(|s| mk(s)).reduce(|a, b| a.compose_desc(&b)).unwrap();
    assert_eq!(composed.to_string(), *result, "composing {sets:?}");
}

#[test]
fn can_compose_unrelated_changes() {
    comp(&["5 0:2", "1 2:0 4", "1 2:0 2 0:2"]);
}

#[test]
fn cancels_insertions_with_deletions() {
    comp(&["2 0:2 2", "2 2:0 2", "4"]);
}

#[test]
fn joins_adjacent_insertions() {
    comp(&["2 0:2 2", "4 0:3 2", "2 0:5 2"]);
}

#[test]
fn joins_adjacent_deletions() {
    comp(&["2 5:0", "1 1:0", "1 6:0"]);
}

#[test]
fn allows_a_delete_to_shadow_multiple_operations() {
    comp(&["2 2:0 0:3", "5:0", "4:0"]);
}

#[test]
fn can_handle_empty_sets() {
    comp(&["", "0:8", "8:0", "", ""]);
}

#[test]
fn can_join_multiple_replaces() {
    comp(&["2 2:2 2:2 2", "1 2:2 2:2 2:2 1", "1 6:6 1"]);
    comp(&["1 2:2 2:2 2:2 1", "2 2:2 2:2 2", "1 6:6 1"]);
    comp(&["1 2:3 3:2 1", "2 3:1 2", "1 5:3 1"]);
}

#[test]
fn throws_for_inconsistent_lengths() {
    for (a, b) in [("2 0:2", "1 0:1"), ("2 0:2", "30 0:1"), ("2 2:0 0:3", "7:0")] {
        let result = std::panic::catch_unwind(|| mk(a).compose_desc(&mk(b)));
        assert!(result.is_err(), "{a} then {b} should fail");
    }
}

// ---------- ChangeDesc mapping ----------

fn over(a: &str, b: &str, result: &str) {
    assert_eq!(mk(a).map_desc(&mk(b), false).to_string(), result, "{a} over {b}");
}

fn under(a: &str, b: &str, result: &str) {
    assert_eq!(mk(a).map_desc(&mk(b), true).to_string(), result, "{a} under {b}");
}

#[test]
fn can_map_over_an_insertion() {
    over("4 0:1", "0:3 4", "7 0:1");
}

#[test]
fn can_map_over_a_deletion() {
    over("4 0:1", "2:0 2", "2 0:1");
}

#[test]
fn orders_insertions() {
    over("2 0:1 2", "2 0:1 2", "3 0:1 2");
    under("2 0:1 2", "2 0:1 2", "2 0:1 3");
}

#[test]
fn can_map_a_deletion_over_an_overlapping_replace() {
    over("2 2:0", "2 1:2 1", "4 1:0");
    under("2 2:0", "2 1:2 1", "4 1:0");
}

#[test]
fn can_handle_changes_after() {
    over("0:1 2:0 8", "6 1:0 0:5 3", "0:1 2:0 12");
}

#[test]
fn joins_deletions() {
    over("5:0 2 3:0 2", "4 4:0 4", "6:0 2");
}

#[test]
fn keeps_insertions_in_deletions() {
    under("2 0:1 2", "4:0", "0:1");
    over("4 0:1 4", "2 4:0 2", "2 0:1 2");
}

#[test]
fn keeps_replacements() {
    over("2 2:2 2", "0:2 6", "4 2:2 2");
    over("2 2:2 2", "3:0 3", "1:2 2");
    over("1 4:4 1", "3 0:2 3", "1 2:4 2 2:0 1");
    over("1 4:4 1", "2 2:0 2", "1 2:4 1");
    over("2 2:2 2", "3 2:0 1", "2 1:2 1");
}

#[test]
fn doesnt_join_replacements() {
    over("2:2 2 2:2", "2 2:0 2", "2:2 2:2");
}

#[test]
fn drops_duplicate_deletion() {
    under("2 2:0 2", "2 2:0 2", "4");
    over("2 2:0 2", "2 2:0 2", "4");
}

#[test]
fn handles_overlapping_replaces() {
    over("1 1:2 1", "1 1:1 1", "2 0:2 1");
    under("1 1:2 1", "1 1:1 1", "1 0:2 2");
    over("1 1:2 2", "1 2:1 1", "1 0:2 2");
    over("2 1:2 1", "1 2:1 1", "2 0:2 1");
    over("2:1 1", "1 2:2", "1:1 2");
    over("1 2:1", "2:2 1", "2 1:1");
}

// ---------- mapPos ----------

enum Opt {
    Assoc(i32),
    Mode(MapMode),
}

fn map(spec: &str, cases: &[(usize, Option<usize>, Opt)]) {
    let set = mk(spec);
    for (from, to, opt) in cases {
        let result = match opt {
            Opt::Assoc(assoc) => Some(set.map_pos(*from, *assoc)),
            Opt::Mode(mode) => set.map_pos_mode(*from, -1, *mode),
        };
        assert_eq!(result, *to, "mapping {from} through {spec}");
    }
}

const D: Opt = Opt::Mode(MapMode::TrackDel);
const A: Opt = Opt::Mode(MapMode::TrackAfter);
const B: Opt = Opt::Mode(MapMode::TrackBefore);
const L: Opt = Opt::Assoc(-1);
const R: Opt = Opt::Assoc(1);

#[test]
fn maps_through_an_insertion() {
    map("4 0:2 4", &[(0, Some(0), L), (4, Some(4), L), (4, Some(6), R), (5, Some(7), L), (8, Some(10), L)]);
}

#[test]
fn maps_through_deletion() {
    map(
        "4 4:0 4",
        &[
            (0, Some(0), L),
            (4, Some(4), L),
            (4, Some(4), D),
            (4, Some(4), B),
            (4, None, A),
            (5, Some(4), L),
            (5, None, D),
            (5, None, B),
            (5, None, A),
            (7, Some(4), L),
            (8, Some(4), L),
            (8, Some(4), D),
            (8, None, B),
            (8, Some(4), A),
            (9, Some(5), L),
            (12, Some(8), L),
        ],
    );
}

#[test]
fn maps_through_multiple_insertions() {
    map(
        "0:2 2 0:2 2 0:2",
        &[
            (0, Some(0), L),
            (0, Some(2), R),
            (1, Some(3), L),
            (2, Some(4), L),
            (2, Some(6), R),
            (3, Some(7), L),
            (4, Some(8), L),
            (4, Some(10), R),
        ],
    );
}

#[test]
fn maps_through_multiple_deletions() {
    map(
        "2:0 2 2:0 2 2:0",
        &[
            (0, Some(0), L),
            (1, Some(0), L),
            (2, Some(0), L),
            (3, Some(1), L),
            (4, Some(2), L),
            (5, Some(2), L),
            (6, Some(2), L),
            (7, Some(3), L),
            (8, Some(4), L),
            (9, Some(4), L),
            (10, Some(4), L),
        ],
    );
}

#[test]
fn maps_through_mixed_edits() {
    map(
        "2 0:2 2:0 0:2 2 2:0 0:2",
        &[
            (0, Some(0), L),
            (2, Some(2), L),
            (2, Some(4), R),
            (3, Some(4), L),
            (4, Some(4), L),
            (4, Some(6), R),
            (5, Some(7), L),
            (6, Some(8), L),
            (7, Some(8), L),
            (8, Some(8), L),
            (8, Some(10), R),
        ],
    );
}

#[test]
fn stays_on_its_own_side_of_replacements() {
    map(
        "2 2:2 2",
        &[
            (2, Some(2), R),
            (2, Some(2), L),
            (2, Some(2), D),
            (2, Some(2), B),
            (2, None, A),
            (3, Some(2), L),
            (3, Some(4), R),
            (3, None, D),
            (3, None, B),
            (3, None, A),
            (4, Some(4), R),
            (4, Some(4), L),
            (4, Some(4), D),
            (4, None, B),
            (4, Some(4), A),
        ],
    );
}

#[test]
fn maps_through_insertions_around_replacements() {
    map(
        "0:1 2:2 0:1",
        &[(0, Some(0), L), (0, Some(1), R), (1, Some(1), L), (1, Some(3), R), (2, Some(3), L), (2, Some(4), R)],
    );
}

#[test]
fn stays_in_between_replacements() {
    map("2:2 2:2", &[(2, Some(2), L), (2, Some(2), R)]);
}

// ---------- ChangeSet ----------

fn doc10() -> Text {
    Text::of(&["0123456789"])
}

#[test]
fn can_create_change_sets() {
    assert_eq!(of(ChangeSpec::insert(5, "hi"), 10).desc().to_string(), "5 0:2 5");
    assert_eq!(of(ChangeSpec::delete(5, 7), 10).desc().to_string(), "5 2:0 3");
    let set = of(
        ChangeSpec::List(vec![
            ChangeSpec::insert(5, "hi"),
            ChangeSpec::insert(5, "ok"),
            ChangeSpec::delete(0, 3),
            ChangeSpec::delete(4, 6),
            ChangeSpec::insert(8, "boo"),
        ]),
        10,
    );
    assert_eq!(set.desc().to_string(), "3:0 1 1:0 0:4 1:0 2 0:3 2");
}

#[test]
fn can_apply_change_sets() {
    assert_eq!(of(ChangeSpec::insert(2, "ok"), 10).apply(&doc10()).to_string(), "01ok23456789");
    assert_eq!(of(ChangeSpec::delete(1, 9), 10).apply(&doc10()).to_string(), "09");
    let set = of(ChangeSpec::List(vec![ChangeSpec::delete(2, 8), ChangeSpec::insert(1, "hi")]), 10);
    assert_eq!(set.apply(&doc10()).to_string(), "0hi189");
}

#[test]
fn can_apply_composed_sets() {
    let set = of(ChangeSpec::insert(8, "ABCD"), 10).compose(&of(ChangeSpec::delete(8, 11), 14));
    assert_eq!(set.apply(&doc10()).to_string(), "01234567D89");
    let a = of(ChangeSpec::List(vec![ChangeSpec::insert(2, "hi"), ChangeSpec::insert(8, "ok")]), 10);
    let b = of(
        ChangeSpec::List(vec![ChangeSpec::insert(4, "!"), ChangeSpec::delete(6, 8), ChangeSpec::insert(12, "?")]),
        14,
    );
    assert_eq!(a.compose(&b).apply(&doc10()).to_string(), "01hi!2367ok?89");
}

#[test]
fn can_clip_inserted_strings_on_compose() {
    let a = of(ChangeSpec::List(vec![ChangeSpec::insert(2, "abc"), ChangeSpec::insert(4, "def")]), 10);
    let b = of(ChangeSpec::delete(4, 8), 16);
    assert_eq!(a.compose(&b).apply(&doc10()).to_string(), "01abef456789");
}

#[test]
fn can_apply_mapped_sets() {
    let set0 = of(ChangeSpec::List(vec![ChangeSpec::insert(5, "hi"), ChangeSpec::delete(8, 10)]), 10);
    let set1 = of(ChangeSpec::List(vec![ChangeSpec::insert(10, "ok"), ChangeSpec::delete(6, 7)]), 10);
    assert_eq!(set0.compose(&set1.map(set0.desc(), false)).apply(&doc10()).to_string(), "01234hi57ok");
}

#[test]
fn can_apply_inverted_sets() {
    let set0 = of(ChangeSpec::List(vec![ChangeSpec::insert(5, "hi"), ChangeSpec::delete(8, 10)]), 10);
    let doc = doc10();
    assert_eq!(set0.invert(&doc).apply(&set0.apply(&doc)).to_string(), doc.to_string());
}

#[test]
fn can_be_iterated() {
    let set = of(ChangeSpec::List(vec![ChangeSpec::insert(4, "ok"), ChangeSpec::delete(6, 8)]), 10);
    let mut result = Vec::new();
    set.iter_changes(|fa, ta, fb, tb, ins| result.push((fa, ta, fb, tb, ins.to_string())), false);
    assert_eq!(result, vec![(4, 4, 4, 6, "ok".to_string()), (6, 8, 8, 8, String::new())]);
    let mut gaps = Vec::new();
    set.iter_gaps(|a, b, len| gaps.push((a, b, len)));
    assert_eq!(gaps, vec![(0, 0, 4), (4, 6, 2), (8, 8, 2)]);
}

#[test]
fn mapping_before_produces_the_same_result_as_mapping_the_other_after() {
    let mut rng = Rng::new(1);
    let total = 100;
    for i in 0..total {
        let size = rng.below(20);
        let count = i / (total / 10) + 1;
        let (a, b) = (r_changes(&mut rng, size, count), r_changes(&mut rng, size, count));
        let (set_a, set_b) = (of(a, size), of(b, size));
        let set_a1 = set_a.map(set_b.desc(), true);
        let set_b1 = set_b.map(set_a.desc(), false);
        let doc = Text::of(&[rng.string(size)]);
        let (ab, ba) = (set_a.compose(&set_b1), set_b.compose(&set_a1));
        assert_eq!(ab.apply(&doc).to_string(), ba.apply(&doc).to_string());
    }
}

#[test]
fn mapping_still_converges_when_mapping_through_multiple_changes() {
    let mut rng = Rng::new(2);
    let total = 100;
    for i in 0..total {
        let size = rng.below(20);
        let count = i / (total / 10) + 1;
        let a = of(r_changes(&mut rng, size, count), size);
        let b = of(r_changes(&mut rng, a.new_length(), count), a.new_length());
        let c = of(r_changes(&mut rng, size, count), size);
        let c_a = c.map(a.desc(), false);
        let c_ab = c_a.map(b.desc(), false);
        let a_c = a.map(c.desc(), true);
        let b_ca = b.map(c_a.desc(), true);
        let doc = Text::of(&[rng.string(size)]);
        assert_eq!(
            a.compose(&b).compose(&c_ab).apply(&doc).to_string(),
            c.compose(&a_c).compose(&b_ca).apply(&doc).to_string()
        );
    }
}

#[test]
fn compose_produces_the_same_result_as_individual_changes() {
    let mut rng = Rng::new(3);
    for _ in 0..100 {
        let size = rng.below(20);
        let doc = Text::of(&[rng.string(size)]);
        let n = rng.below(5) + 1;
        let a = of(r_changes(&mut rng, size, n), size);
        let n = rng.below(6);
        let b = of(r_changes(&mut rng, a.new_length(), n), a.new_length());
        assert_eq!(b.apply(&a.apply(&doc)).to_string(), a.compose(&b).apply(&doc).to_string());
    }
}

#[test]
fn composing_is_associative() {
    let mut rng = Rng::new(4);
    for _ in 0..100 {
        let size = rng.below(20);
        let doc = Text::of(&[rng.string(size)]);
        let n = rng.below(5) + 1;
        let a = of(r_changes(&mut rng, size, n), size);
        let n = rng.below(6);
        let b = of(r_changes(&mut rng, a.new_length(), n), a.new_length());
        let n = rng.below(5) + 1;
        let c = of(r_changes(&mut rng, b.new_length(), n), b.new_length());
        let left = a.compose(&b).compose(&c);
        let right = a.compose(&b.compose(&c));
        assert_eq!(left.apply(&doc).to_string(), right.apply(&doc).to_string());
    }
}

#[test]
fn survives_random_sequences_of_changes() {
    let mut rng = Rng::new(5);
    let doc10 = doc10();
    for _ in 0..50 {
        let mut doc = doc10.clone();
        let mut txt = doc.to_string();
        let mut all: Vec<ChangeSet> = Vec::new();
        let mut inv: Vec<ChangeSet> = Vec::new();
        for _ in 0..50 {
            let change = r_change(&mut rng, doc.len());
            let ChangeSpec::Change { from, to, insert } = &change else { unreachable!() };
            let to = to.unwrap_or(*from);
            let insert = match insert {
                Some(codemirror_state::Insert::Str(s)) => s.clone(),
                _ => String::new(),
            };
            txt = format!("{}{}{}", &txt[..*from], insert, &txt[to..]);
            let set = of(change.clone(), doc.len());
            inv.push(set.invert(&doc));
            doc = set.apply(&doc);
            all.push(set);
            assert_eq!(doc.to_string(), txt);
        }
        let composed = all.iter().fold(of(ChangeSpec::none(), doc10.len()), |a, b| a.compose(b));
        assert_eq!(composed.apply(&doc10).to_string(), txt);
        assert_eq!(composed.invert(&doc10).apply(&doc).to_string(), doc10.to_string());
        for set in inv.iter().rev() {
            doc = set.apply(&doc);
        }
        assert_eq!(doc.to_string(), doc10.to_string());
    }
}

#[test]
fn multibyte_text_maps_by_bytes() {
    // Not in CodeMirror's suite: positions are UTF-8 byte offsets here.
    let doc = Text::from("héllo");
    let set = of(ChangeSpec::replace(1, 3, "e"), doc.len());
    assert_eq!(set.apply(&doc).to_string(), "hello");
    assert_eq!(set.map_pos(3, -1), 2);
    assert_eq!(set.invert(&doc).apply(&set.apply(&doc)).to_string(), "héllo");
}

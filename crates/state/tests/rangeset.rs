//! Port of CodeMirror's `test/test-rangeset.ts`.

use std::sync::{Arc, LazyLock};

use codemirror_state::{
    ChangeSet, ChangeSpec, Range, RangeComparator, RangeSet, RangeSetUpdate, RangeValue, SpanIterator,
};

struct Value {
    start_side: i64,
    end_side: i64,
    point: bool,
    name: Option<String>,
    pos: Option<usize>,
}

impl RangeValue for Value {
    fn value_eq(&self, other: &Self) -> bool {
        other.name == self.name
    }
    fn start_side(&self) -> i64 {
        self.start_side
    }
    fn end_side(&self) -> i64 {
        self.end_side
    }
    fn point(&self) -> bool {
        self.point
    }
}

fn names(values: &[Arc<Value>]) -> String {
    values
        .iter()
        .filter(|v| v.name.is_some() || v.point)
        .map(|v| v.name.clone().unwrap_or_else(|| "POINT".into()))
        .collect::<Vec<_>>()
        .join("/")
}

#[derive(Default, Clone)]
struct Spec {
    start_side: Option<i64>,
    end_side: Option<i64>,
    point: bool,
    name: Option<&'static str>,
    pos: Option<usize>,
}

fn name(n: &'static str) -> Spec {
    Spec { name: Some(n), ..Spec::default() }
}

fn sides(start: i64, end: i64) -> Spec {
    Spec { start_side: Some(start), end_side: Some(end), ..Spec::default() }
}

fn start(side: i64) -> Spec {
    Spec { start_side: Some(side), ..Spec::default() }
}

fn mk(from: usize, to: usize, spec: Spec) -> Range<Value> {
    let empty = from == to;
    let value = Value {
        start_side: spec.start_side.unwrap_or(1),
        end_side: spec.end_side.or(spec.start_side).unwrap_or(if empty { 1 } else { -1 }),
        point: empty || spec.point,
        name: spec.name.map(String::from),
        pos: spec.pos,
    };
    Range::new(from, to, value)
}

fn mk_set(ranges: Vec<Range<Value>>) -> RangeSet<Value> {
    RangeSet::of(ranges, false)
}

fn sorted(mut ranges: Vec<Range<Value>>) -> Vec<Range<Value>> {
    ranges.sort_by_key(|r| r.from);
    ranges
}

fn change_set(changes: &[(usize, usize, usize)]) -> ChangeSet {
    let mut collect = Vec::new();
    for &(from, to, len) in changes {
        if len > 0 {
            collect.push(ChangeSpec::replace(from, to, "x".repeat(len)));
        }
        if from < to {
            collect.push(ChangeSpec::delete(from, to));
        }
    }
    ChangeSet::of(collect, 5100, None)
}

fn small_ranges() -> Vec<Range<Value>> {
    (0..5000).map(|i| mk(i, i + 1 + (i % 4), Spec { pos: Some(i), ..Spec::default() })).collect()
}

static SET0: LazyLock<RangeSet<Value>> = LazyLock::new(|| mk_set(small_ranges()));

fn check_set(set: &RangeSet<Value>) {
    let mut count = 0;
    set.between(0, set.length(), |from, _, value| {
        count += 1;
        if let Some(pos) = value.pos {
            assert_eq!(from, pos);
        }
        true
    });
    assert_eq!(count, set.size());
}

fn filtered(set: &RangeSet<Value>, filter: &dyn Fn(usize, usize, &Value) -> bool) -> RangeSet<Value> {
    set.update(RangeSetUpdate { filter: Some(filter), ..RangeSetUpdate::default() })
}

fn added(set: &RangeSet<Value>, add: Vec<Range<Value>>) -> RangeSet<Value> {
    set.update(RangeSetUpdate { add, ..RangeSetUpdate::default() })
}

#[test]
fn divides_a_set_into_chunks_and_layers() {
    let mut ranges = small_ranges();
    ranges.push(mk(1000, 4000, Spec { pos: Some(1000), ..Spec::default() }));
    ranges.push(mk(2000, 3000, Spec { pos: Some(2000), ..Spec::default() }));
    let set = mk_set(sorted(ranges));
    assert_eq!(set.size(), 5002);
    assert!(set.chunk_count() > 1);
    assert!(set.has_next_layer());
    check_set(&set);
}

#[test]
fn complains_about_misordered_ranges() {
    let msg = |f: fn()| {
        let err = std::panic::catch_unwind(f).expect_err("should panic");
        err.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| err.downcast_ref::<String>().cloned()).unwrap()
    };
    assert!(msg(|| {
        mk_set(vec![mk(8, 9, Spec::default()), mk(7, 10, Spec::default())]);
    })
    .contains("sorted"));
    assert!(msg(|| {
        mk_set(vec![mk(1, 1, start(1)), mk(1, 1, start(-1))]);
    })
    .contains("sorted"));
}

// ---------- update ----------

#[test]
fn can_add_ranges() {
    let set = added(&SET0, vec![mk(4000, 4000, Spec { pos: Some(4000), ..Spec::default() })]);
    assert_eq!(set.size(), 5001);
    assert!(set.shares_first_chunk(&SET0));
}

#[test]
fn can_add_a_large_amount_of_ranges() {
    let ranges = (0..4000).step_by(2).map(|i| mk(i, i, Spec::default())).collect();
    let set = added(&SET0, ranges);
    assert_eq!(set.size(), 2000 + SET0.size());
    check_set(&set);
}

#[test]
fn can_filter_ranges() {
    let set = filtered(&SET0, &|from, _, _| from >= 2500);
    assert_eq!(set.size(), 2500);
    assert!(set.chunk_count() < SET0.chunk_count());
    check_set(&set);
}

#[test]
fn can_filter_all_over() {
    let set = filtered(&SET0, &|from, _, _| from % 200 >= 100);
    assert_eq!(set.size(), 2500);
    check_set(&set);
}

#[test]
fn collapses_the_chunks_when_removing_almost_all_ranges() {
    let set = filtered(&SET0, &|from, _, _| from == 500 || from == 501);
    assert_eq!(set.size(), 2);
    assert_eq!(set.chunk_count(), 1);
}

#[test]
fn calls_filter_on_precisely_those_ranges_touching_the_filter_range() {
    let ranges = (0..1000).map(|i| mk(i, i + 1, Spec { pos: Some(i), ..Spec::default() })).collect();
    let set = mk_set(ranges);
    let called = std::cell::RefCell::new(Vec::new());
    let filter = |from: usize, to: usize, _: &Value| {
        called.borrow_mut().push((from, to));
        true
    };
    set.update(RangeSetUpdate {
        filter: Some(&filter),
        filter_from: 400,
        filter_to: Some(600),
        ..RangeSetUpdate::default()
    });
    let called = called.into_inner();
    assert_eq!(called.len(), 202);
    for (j, i) in (399..=600).enumerate() {
        assert_eq!(called[j], (i, i + 1));
    }
}

#[test]
fn returns_the_empty_set_when_filter_removes_everything() {
    assert!(filtered(&SET0, &|_, _, _| false).is_empty());
}

// ---------- map ----------

fn test_map(positions: Vec<Range<Value>>, changes: &[(usize, usize, usize)], expected: &[(usize, usize)]) {
    let set = mk_set(positions);
    let mapped = set.map(change_set(changes).desc());
    let mut out = Vec::new();
    let mut iter = mapped.iter(0);
    while iter.value().is_some() {
        out.push((iter.from(), iter.to()));
        iter.next();
    }
    assert_eq!(out, expected);
}

fn d() -> Spec {
    Spec::default()
}

#[test]
fn can_map_through_changes() {
    test_map(
        vec![mk(1, 1, d()), mk(4, 4, d()), mk(10, 10, d())],
        &[(0, 0, 1), (2, 3, 0), (8, 8, 20)],
        &[(2, 2), (4, 4), (30, 30)],
    );
}

#[test]
fn takes_inclusivity_into_account() {
    test_map(vec![mk(1, 2, sides(-1, 1))], &[(1, 1, 2), (2, 2, 2)], &[(1, 6)]);
}

#[test]
fn uses_side_to_determine_mapping_of_points() {
    test_map(vec![mk(1, 1, sides(-1, -1)), mk(1, 1, sides(1, 1))], &[(1, 1, 2)], &[(1, 1), (3, 3)]);
}

#[test]
fn defaults_to_exclusive_on_both_sides() {
    test_map(vec![mk(1, 2, d())], &[(1, 1, 2), (4, 4, 2)], &[(3, 4)]);
}

#[test]
fn drops_point_ranges() {
    test_map(vec![mk(1, 2, d())], &[(1, 2, 0), (1, 1, 1)], &[]);
}

#[test]
fn drops_ranges_in_deleted_regions() {
    test_map(vec![mk(1, 2, d()), mk(3, 3, d())], &[(0, 4, 0)], &[]);
}

#[test]
fn shrinks_ranges() {
    test_map(vec![mk(2, 4, d()), mk(2, 8, d()), mk(6, 8, d())], &[(3, 7, 0)], &[(2, 3), (2, 4), (3, 4)]);
}

#[test]
fn leaves_point_ranges_on_change_boundaries() {
    test_map(vec![mk(2, 2, d()), mk(4, 4, d())], &[(2, 4, 6)], &[(2, 2), (8, 8)]);
}

#[test]
fn can_collapse_chunks() {
    let smaller = SET0.map(change_set(&[(30, 4500, 0)]).desc());
    assert!(smaller.chunk_count() < SET0.chunk_count());
    let empty = smaller.map(change_set(&[(0, 1000, 0)]).desc());
    assert!(empty.is_empty());
}

#[test]
fn rearranges_points_that_change_order() {
    let set = mk_set(vec![
        mk(1, 1, Spec { name: Some("a"), start_side: Some(1), ..d() }),
        mk(2, 2, Spec { name: Some("b"), start_side: Some(-1), ..d() }),
    ])
    .map(change_set(&[(1, 2, 0)]).desc());
    let mut iter = set.iter(0);
    assert_eq!(iter.from(), 1);
    assert_eq!(iter.value().unwrap().name.as_deref(), Some("b"));
    iter.next();
    assert_eq!(iter.from(), 1);
    assert_eq!(iter.value().unwrap().name.as_deref(), Some("a"));
    iter.next();
    assert!(iter.value().is_none());
}

#[test]
fn spills_ranges_that_overlap() {
    let set = mk_set(vec![
        mk(0, 1, Spec { name: Some("a"), end_side: Some(1), ..d() }),
        mk(2, 3, Spec { name: Some("b"), start_side: Some(-1), ..d() }),
    ])
    .map(change_set(&[(1, 2, 0)]).desc())
    .map(change_set(&[(1, 1, 1)]).desc());
    assert!(set.has_next_layer());
}

#[test]
fn spills_ranges_that_mapping_puts_out_of_order() {
    test_map(
        vec![mk(0, 1, sides(-1, 1)), mk(2, 3, sides(-1, 1)), mk(4, 4, start(-2))],
        &[(0, 4, 0)],
        &[(0, 0), (0, 0), (0, 0)],
    );
}

#[test]
fn rearranges_a_point_that_moves_before_several_others() {
    let set = mk_set(vec![
        mk(1, 1, Spec { name: Some("a"), start_side: Some(0), ..d() }),
        mk(1, 1, Spec { name: Some("b"), start_side: Some(1), ..d() }),
        mk(1, 1, Spec { name: Some("c"), start_side: Some(1), ..d() }),
        mk(7, 7, Spec { name: Some("d"), start_side: Some(0), ..d() }),
    ])
    .map(change_set(&[(1, 7, 0)]).desc());
    let mut names = Vec::new();
    let mut iter = set.iter(0);
    while let Some(value) = iter.value() {
        assert_eq!((iter.from(), iter.to()), (1, 1));
        names.push(value.name.clone().unwrap());
        iter.next();
    }
    assert_eq!(names.join(","), "a,d,b,c");
}

#[test]
fn keeps_chunk_offsets_relative_when_reordering_points() {
    test_map(
        vec![mk(10, 10, start(0)), mk(10, 10, start(1)), mk(10, 10, start(1)), mk(16, 16, start(0))],
        &[(10, 16, 0)],
        &[(10, 10), (10, 10), (10, 10), (10, 10)],
    );
}

#[test]
fn keeps_a_point_after_a_range_that_starts_before_it() {
    test_map(
        vec![mk(0, 10, sides(-1, 1)), mk(12, 20, sides(-1, 1)), mk(25, 25, start(0))],
        &[(15, 25, 0)],
        &[(0, 10), (12, 15), (15, 15)],
    );
}

// ---------- compare ----------

#[derive(Default)]
struct Comparator {
    ranges: Vec<usize>,
}

impl Comparator {
    fn add_range(&mut self, from: usize, to: usize) {
        match self.ranges.last_mut() {
            Some(last) if *last == from => *last = to,
            _ => self.ranges.extend([from, to]),
        }
    }
}

impl RangeComparator<Value> for Comparator {
    fn compare_range(&mut self, from: usize, to: usize, _: &[Arc<Value>], _: &[Arc<Value>]) {
        self.add_range(from, to);
    }
    fn compare_point(&mut self, from: usize, to: usize, _: Option<&Arc<Value>>, _: Option<&Arc<Value>>) {
        self.add_range(from, to);
    }
}

type Filter = Box<dyn Fn(usize, usize, &Value) -> bool>;

#[derive(Default)]
struct Update {
    changes: Vec<(usize, usize, usize)>,
    add: Vec<Range<Value>>,
    filter: Option<Filter>,
}

fn test_compare(set: RangeSet<Value>, update: Update, expected: &[usize]) {
    let doc_changes = change_set(&update.changes);
    let mut new_set = set.clone();
    if !update.changes.is_empty() {
        new_set = new_set.map(doc_changes.desc());
    }
    if update.filter.is_some() || !update.add.is_empty() {
        new_set = new_set.update(RangeSetUpdate {
            add: update.add,
            filter: update.filter.as_deref(),
            ..RangeSetUpdate::default()
        });
    }
    let mut comp = Comparator::default();
    RangeSet::compare(&[set], &[new_set], doc_changes.desc(), &mut comp, -1);
    assert_eq!(comp.ranges, expected);
}

#[test]
fn notices_added_ranges() {
    test_compare(
        mk_set(vec![mk(2, 4, name("a")), mk(8, 11, name("a"))]),
        Update { add: vec![mk(3, 9, name("b")), mk(106, 107, name("b"))], ..Update::default() },
        &[3, 9, 106, 107],
    );
}

#[test]
fn notices_deleted_ranges() {
    test_compare(
        mk_set(vec![mk(4, 6, name("a")), mk(5, 7, name("b")), mk(6, 8, name("c")), mk(20, 30, name("d"))]),
        Update { filter: Some(Box::new(|from, _, _| from != 5 && from != 20)), ..Update::default() },
        &[5, 7, 20, 30],
    );
}

#[test]
fn recognizes_identical_ranges() {
    test_compare(
        mk_set(vec![mk(0, 50, name("a"))]),
        Update { add: vec![mk(10, 40, name("a"))], filter: Some(Box::new(|_, _, _| false)), ..Update::default() },
        &[0, 10, 40, 50],
    );
}

#[test]
fn skips_changes() {
    test_compare(
        mk_set(vec![mk(0, 20, name("a"))]),
        Update { changes: vec![(5, 15, 20)], filter: Some(Box::new(|_, _, _| false)), ..Update::default() },
        &[0, 5, 25, 30],
    );
}

#[test]
fn ignores_identical_sub_nodes() {
    let ranges = (0..1000).map(|i| mk(i, i + 1, name("a"))).collect();
    test_compare(
        mk_set(ranges),
        Update { changes: vec![(900, 1000, 0)], add: vec![mk(850, 860, name("b"))], ..Update::default() },
        &[850, 860],
    );
}

#[test]
fn ignores_changes_in_points() {
    let mut ranges = vec![mk(3, 997, Spec { point: true, ..d() })];
    ranges.extend((0..1000).step_by(2).map(|i| mk(i, i + 1, name("a"))));
    test_compare(mk_set(sorted(ranges)), Update { changes: vec![(300, 500, 100)], ..Update::default() }, &[]);
}

#[test]
fn notices_adding_a_point() {
    test_compare(
        mk_set(vec![mk(3, 50, Spec { point: true, ..d() })]),
        Update { add: vec![mk(40, 80, Spec { point: true, ..d() })], ..Update::default() },
        &[50, 80],
    );
}

#[test]
fn notices_removing_a_point() {
    test_compare(
        mk_set(vec![mk(3, 50, Spec { point: true, ..d() })]),
        Update { filter: Some(Box::new(|_, _, _| false)), ..Update::default() },
        &[3, 50],
    );
}

#[test]
fn can_handle_multiple_changes() {
    let names: Vec<&'static str> = (0usize..200)
        .step_by(2)
        .map(|i| Box::leak(format!("{}-{}", i, i + 1 + i.div_ceil(50)).into_boxed_str()) as &'static str)
        .collect();
    let ranges = (0..200).step_by(2).zip(&names).map(|(i, n)| mk(i, i + 1 + i.div_ceil(50), name(n))).collect();
    test_compare(
        mk_set(ranges),
        Update {
            changes: vec![(0, 0, 50), (50, 100, 0), (150, 200, 0)],
            filter: Some(Box::new(|from, _, _| from % 50 > 0)),
            ..Update::default()
        },
        &[50, 51, 100, 103, 148, 153],
    );
}

#[test]
fn reports_point_decorations_with_different_cover() {
    test_compare(
        mk_set(vec![
            mk(0, 4, sides(1, -1)),
            mk(1, 3, Spec { point: true, start_side: Some(-1), end_side: Some(1), ..d() }),
        ]),
        Update { changes: vec![(2, 4, 0)], ..Update::default() },
        &[1, 2],
    );
}

// ---------- spans ----------

#[derive(Default)]
struct Builder {
    spans: Vec<String>,
}

impl SpanIterator<Value> for Builder {
    fn span(&mut self, from: usize, to: usize, active: &[Arc<Value>], _: usize) {
        let name = names(active);
        self.spans.push(format!("{}{}", to - from, if name.is_empty() { String::new() } else { format!("={name}") }));
    }
    fn point(&mut self, from: usize, to: usize, value: &Arc<Value>, _: &[Arc<Value>], _: usize, _: usize) {
        let size = if to > from { format!("{}=", to - from) } else { String::new() };
        let label = value.name.as_ref().map_or("ø".to_string(), |n| format!("[{n}]"));
        self.spans.push(format!("{size}{label}"));
    }
}

fn test_spans(sets: &[RangeSet<Value>], start: usize, end: usize, expected: &str) {
    let mut builder = Builder::default();
    RangeSet::spans(sets, start, end, &mut builder, -1);
    assert_eq!(builder.spans.join(" "), expected);
}

#[test]
fn separates_the_range_in_covering_spans() {
    let set = mk_set(vec![mk(3, 8, name("one")), mk(5, 8, name("two")), mk(10, 12, name("three"))]);
    test_spans(&[set], 0, 15, "3 2=one 3=two/one 2 2=three 3");
}

#[test]
fn can_retrieve_a_limited_range() {
    let span_names: Vec<&'static str> =
        (0..100).map(|i| Box::leak(format!("span{i}").into_boxed_str()) as &'static str).collect();
    let mut decos = vec![mk(0, 200, name("wide"))];
    for (i, n) in span_names.iter().enumerate() {
        decos.push(mk(i * 2, i * 2 + 2, name(n)));
    }
    let set = mk_set(decos);
    let (start, end) = (20, 26);
    let mut expected = Vec::new();
    let mut pos = start;
    while pos < end {
        let step = if pos % 2 == 1 { 1 } else { 2 };
        expected.push(format!("{}=span{}/wide", end.min(pos + step) - pos, pos / 2));
        pos += step;
    }
    test_spans(&[set], start, end, &expected.join(" "));
}

#[test]
fn reads_from_multiple_sets_at_once() {
    let one = mk_set(vec![mk(2, 3, name("x")), mk(5, 10, name("y")), mk(10, 12, name("z"))]);
    let two = mk_set(vec![mk(0, 6, name("a")), mk(10, 12, name("b"))]);
    test_spans(&[one, two], 0, 12, "2=a 1=x/a 2=a 1=y/a 4=y 2=z/b");
}

#[test]
fn orders_active_ranges_by_origin_set() {
    let one = mk_set(vec![mk(2, 10, name("a")), mk(20, 30, name("a"))]);
    let two = mk_set(vec![mk(3, 4, name("b")), mk(8, 12, name("b")), mk(18, 22, name("b"))]);
    let three = mk_set(vec![mk(0, 25, name("c"))]);
    test_spans(&[one, two, three], 0, 25, "2=c 1=a/c 1=a/b/c 4=a/c 2=a/b/c 2=b/c 6=c 2=b/c 2=a/b/c 3=a/c");
}

#[test]
fn doesnt_get_confused_by_same_place_points() {
    test_spans(&[mk_set(vec![mk(1, 1, name("a")), mk(1, 1, name("b")), mk(1, 1, name("c"))])], 0, 2, "1 [a] [b] [c] 1");
}

#[test]
fn properly_resyncs_active_ranges_after_points() {
    let set = mk_set(vec![
        mk(0, 20, name("r1")),
        mk(1, 10, name("r2")),
        mk(3, 12, Spec { name: Some("p"), point: true, ..d() }),
        mk(4, 8, name("r3")),
        mk(5, 20, name("r4")),
    ]);
    test_spans(&[set], 0, 20, "1=r1 2=r2/r1 9=[p] 8=r4/r1");
}

struct Positions(Vec<usize>);

impl SpanIterator<Value> for Positions {
    fn span(&mut self, from: usize, to: usize, _: &[Arc<Value>], _: usize) {
        self.0.extend([from, to]);
    }
    fn point(&mut self, from: usize, to: usize, _: &Arc<Value>, _: &[Arc<Value>], _: usize, _: usize) {
        self.0.extend([from, to]);
    }
}

#[test]
fn doesnt_split_spans_on_ignored_ranges() {
    let set = mk_set(vec![mk(0, 10, name("a")), mk(20, 30, Spec { name: Some("b"), point: true, ..d() })]);
    let mut ranges = Positions(Vec::new());
    RangeSet::spans(&[set], 0, 30, &mut ranges, 0);
    assert_eq!(ranges.0, vec![0, 20, 20, 30]);
}

struct CountPoints(usize);

impl SpanIterator<Value> for CountPoints {
    fn span(&mut self, _: usize, _: usize, _: &[Arc<Value>], _: usize) {}
    fn point(&mut self, _: usize, _: usize, _: &Arc<Value>, _: &[Arc<Value>], _: usize, _: usize) {
        self.0 += 1;
    }
}

#[test]
fn omits_points_that_are_covered_by_the_previous_point() {
    let set =
        mk_set(vec![mk(0, 4, Spec { point: true, ..d() }), mk(1, 5, name("a")), mk(2, 4, Spec { point: true, ..d() })]);
    let mut points = CountPoints(0);
    RangeSet::spans(&[set], 0, 10, &mut points, -1);
    assert_eq!(points.0, 1);
}

#[test]
fn puts_smaller_spans_inside_bigger_ones_with_the_same_rank() {
    let set = mk_set(vec![mk(0, 3, name("x")), mk(0, 1, name("a")), mk(1, 2, name("b")), mk(2, 3, name("c"))]);
    test_spans(&[set], 0, 3, "1=a/x 1=b/x 1=c/x");
}

// ---------- iter ----------

#[test]
fn iterates_over_ranges() {
    let mut ranges = small_ranges();
    ranges.push(mk(1000, 4000, Spec { pos: Some(1000), ..d() }));
    ranges.push(mk(2000, 3000, Spec { pos: Some(2000), ..d() }));
    let set = mk_set(sorted(ranges));
    let mut count = 0;
    let mut iter = set.iter(0);
    while iter.value().is_some() {
        let expected = if count > 2001 {
            count - 2
        } else if count > 1000 {
            count - 1
        } else {
            count
        };
        assert_eq!(iter.from(), expected);
        iter.next();
        count += 1;
    }
    assert_eq!(count, 5002);
}

#[test]
fn can_iterate_over_a_subset() {
    let mut count = 0;
    let mut iter = SET0.iter(1000);
    while iter.value().is_some() {
        if iter.from() > 2000 {
            break;
        }
        assert_eq!(iter.to(), iter.from() + 1 + (iter.from() % 4));
        iter.next();
        count += 1;
    }
    assert_eq!(count, 1003);
}

// ---------- between ----------

#[test]
fn between_iterates_over_ranges() {
    let mut found = 0;
    SET0.between(100, 200, |from, to, _| {
        assert_eq!(to, from + 1 + (from % 4));
        assert!(to >= 100);
        assert!(from <= 200);
        found += 1;
        true
    });
    assert_eq!(found, 103);
}

#[test]
fn returns_ranges_in_a_zero_length_set() {
    let set = mk_set(vec![mk(0, 0, d())]);
    let mut found = Vec::new();
    set.between(0, 0, |from, to, _| {
        found.extend([from, to]);
        true
    });
    assert_eq!(found, vec![0, 0]);
}

#[test]
fn joins_sets() {
    // Not in CodeMirror's suite.
    let a = mk_set(vec![mk(0, 2, name("a"))]);
    let b = mk_set(vec![mk(1, 3, name("b"))]);
    let joined = RangeSet::join(&[a, b]);
    assert_eq!(joined.size(), 2);
    let mut builder = Builder::default();
    RangeSet::spans(&[joined], 0, 3, &mut builder, -1);
    assert_eq!(builder.spans.len(), 3);
}

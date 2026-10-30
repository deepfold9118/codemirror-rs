//! Port of CodeMirror's `test/test-heightmap.ts`.

use std::sync::Arc;

use codemirror_state::{Range, Text};

use super::*;
use crate::decoration::{Decoration, MarkSpec, ReplaceSpec, WidgetSpec, WidgetType};

fn o(doc: &Text) -> HeightOracle {
    let mut oracle = HeightOracle::new(false);
    oracle.set_doc(doc);
    oracle
}

fn mk(text: &Text, deco: Vec<Range<Decoration>>) -> Box<HeightMap> {
    HeightMap::empty().apply_changes(
        &[Decoration::set(deco, true)],
        &Text::empty(),
        &mut o(text),
        &[ChangedRange::new(0, 0, 0, text.len())],
    )
}

fn doc(line_len: &[usize]) -> Text {
    let lines: Vec<String> = line_len.iter().map(|&n| "x".repeat(n)).collect();
    Text::of(&lines)
}

struct MyWidget(f64);

impl WidgetType for MyWidget {
    fn eq(&self, other: &dyn WidgetType) -> bool {
        (other as &dyn std::any::Any).downcast_ref::<MyWidget>().is_some_and(|o| o.0 == self.0)
    }
    fn estimated_height(&self) -> f64 {
        self.0
    }
}

struct NoHeightWidget;

impl WidgetType for NoHeightWidget {}

fn widget(height: f64, side: i64, block: bool) -> Decoration {
    Decoration::widget(WidgetSpec { widget: Arc::new(MyWidget(height)), side, inline_order: false, block })
}

fn replace() -> Decoration {
    Decoration::replace(ReplaceSpec::default())
}

fn replace_block(height: f64) -> Decoration {
    Decoration::replace(ReplaceSpec { widget: Some(Arc::new(MyWidget(height))), block: true, ..Default::default() })
}

fn changed(from_a: usize, to_a: usize, from_b: usize, to_b: usize) -> ChangedRange {
    ChangedRange::new(from_a, to_a, from_b, to_b)
}

fn by_p(map: &HeightMap, pos: usize, oracle: &HeightOracle) -> BlockInfo {
    map.line_at(pos as f64, QueryType::ByPos, oracle, 0.0, 0)
}

fn by_h(map: &HeightMap, height: f64, oracle: &HeightOracle) -> BlockInfo {
    map.line_at(height, QueryType::ByHeight, oracle, 0.0, 0)
}

fn eq_block(a: &BlockInfo, b: &BlockInfo) -> bool {
    a.from == b.from && a.to() == b.to() && a.top == b.top && a.bottom() == b.bottom()
}

fn measured(from: usize, heights: &[f64]) -> MeasuredHeights {
    MeasuredHeights::new(from, heights.to_vec())
}

fn update(map: Box<HeightMap>, oracle: &HeightOracle, m: &mut MeasuredHeights) -> Box<HeightMap> {
    map.update_height(oracle, 0, false, Some(m))
}

#[test]
fn starts_empty() {
    let empty = HeightMap::empty();
    assert_eq!(empty.length(), 0);
    assert_eq!(empty.size(), 1);
}

#[test]
fn grows_to_match_the_document() {
    assert_eq!(mk(&doc(&[10, 10, 8]), vec![]).length(), 30);
}

#[test]
fn separates_lines_with_decorations_on_them() {
    let map = mk(&doc(&[10, 10, 20, 5]), vec![widget(20.0, 0, false).at(5), replace().range(25, 46)]);
    assert_eq!(map.length(), 48);
    assert_eq!(map.to_string(), "line(10:20) gap(10) line(26-21)");
}

#[test]
fn ignores_irrelevant_decorations() {
    let no_height =
        Decoration::widget(WidgetSpec { widget: Arc::new(NoHeightWidget), side: 0, inline_order: false, block: false });
    let mark = Decoration::mark(MarkSpec { class: Some("ahah".into()), ..Default::default() });
    let map = mk(&doc(&[10, 10, 20, 5]), vec![no_height.at(5), mark.range(25, 46)]);
    assert_eq!(map.length(), 48);
    assert_eq!(map.to_string(), "gap(48)");
}

#[test]
fn drops_decorations_from_the_tree_when_they_are_deleted() {
    let text = doc(&[20]);
    let mut map = mk(&text, vec![widget(20.0, 0, false).at(5)]);
    assert_eq!(map.to_string(), "line(20:20)");
    map = map.apply_changes(&[], &text, &mut o(&text), &[changed(5, 5, 5, 5)]);
    assert_eq!(map.to_string(), "line(20)");
}

#[test]
fn updates_the_length_of_replaced_decorations_for_changes() {
    let text = doc(&[20]);
    let mut map = mk(&text, vec![replace().range(5, 15)]);
    map = map.apply_changes(
        &[Decoration::set(vec![replace().range(5, 10)], false)],
        &text,
        &mut o(&text.replace(7, 12, &Text::empty())),
        &[changed(7, 12, 7, 7)],
    );
    assert_eq!(map.to_string(), "line(15-5)");
}

#[test]
fn stores_information_about_block_widgets() {
    let text = doc(&[3, 3, 3]);
    let oracle = o(&text);
    let mut map =
        mk(&text, vec![widget(10.0, -1, true).at(0), widget(13.0, -1, true).at(0), widget(5.0, 1, true).at(3)]);
    assert_eq!(map.to_string(), "block(0)-block(0)-line(3)-block(0) gap(7)");
    assert_eq!(map.height(), 28.0 + 3.0 * oracle.line_height);
    let line = by_p(&map, 0, &oracle);
    let heights: Vec<f64> = line.children().unwrap().iter().map(|b| b.height).collect();
    assert_eq!(heights, [10.0, 13.0, oracle.line_height, 5.0]);
    assert_eq!(by_p(&map, 4, &oracle).top, 28.0 + oracle.line_height);
    map = update(map, &oracle, &mut measured(0, &[8.0, 12.0, 10.0, 20.0, 40.0, 20.0]));
    assert_eq!(map.to_string(), "block(0)-block(0)-line(3)-block(0) line(3) line(3)");
    assert_eq!(map.height(), 110.0);
}

#[test]
fn stores_information_about_block_ranges() {
    let text = doc(&[3, 3, 3, 3, 3, 3]);
    let mut map = mk(
        &text,
        vec![
            widget(10.0, -1, true).at(4),
            replace_block(40.0).range(4, 11),
            widget(15.0, 1, true).at(11),
            // Widgets after the start or before the end of an inclusive
            // range get covered.
            widget(20.0, 1, true).at(16),
            replace_block(50.0).range(16, 19),
            widget(10.0, -1, true).at(19),
        ],
    );
    assert_eq!(map.to_string(), "gap(3) block(0)-block(7)-block(0) gap(3) block(3) gap(3)");
    map = update(map, &o(&text), &mut measured(4, &[5.0, 5.0, 5.0, 5.0, 10.0, 5.0]));
    assert_eq!(map.height(), o(&text).line_height + 35.0);
}

#[test]
fn handles_empty_lines_correctly() {
    let text = doc(&[0, 0, 0, 0, 0]);
    let mut map =
        mk(&text, vec![widget(10.0, -1, true).at(1), replace_block(20.0).range(2, 2), widget(30.0, 1, true).at(3)]);
    assert_eq!(map.to_string(), "gap(0) block(0)-line(0) block(0) line(0)-block(0) gap(0)");
    map = map.apply_changes(&[], &text, &mut o(&text.replace(1, 3, &Text::of(&["y"]))), &[changed(1, 3, 1, 2)]);
    assert_eq!(map.to_string(), "gap(3)");
}

#[test]
fn joins_ranges() {
    let text = doc(&[10, 10, 10, 10]);
    let mut map = mk(&text, vec![replace().range(16, 27)]);
    assert_eq!(map.to_string(), "gap(10) line(21-11) gap(10)");
    map = map.apply_changes(&[], &text, &mut o(&text.replace(5, 38, &Text::of(&["yyy"]))), &[changed(5, 38, 5, 8)]);
    assert_eq!(map.to_string(), "gap(13)");
}

#[test]
fn joins_lines() {
    let text = doc(&[10, 10, 10]);
    let mut map = mk(&text, vec![replace().range(2, 5), widget(20.0, 0, false).at(24)]);
    assert_eq!(map.to_string(), "line(10-3) gap(10) line(10:20)");
    map = map.apply_changes(
        &[Decoration::set(vec![replace().range(2, 5), widget(20.0, 0, false).at(12)], false)],
        &text,
        &mut o(&text.replace(10, 22, &Text::empty())),
        &[changed(10, 22, 10, 10)],
    );
    assert_eq!(map.to_string(), "line(20-3:20)");
}

#[test]
fn materializes_lines_for_measured_heights() {
    let text = doc(&[10, 10, 10, 10]);
    let oracle = o(&text);
    let map = update(mk(&text, vec![]), &oracle, &mut measured(11, &[28.0, 14.0, 5.0]));
    assert_eq!(map.to_string(), "gap(10) line(10) line(10) line(10)");
    assert_eq!(map.height(), 61.0);
}

#[test]
fn doesnt_set_the_height_changed_flag_for_small_within_line_replacements() {
    let text = doc(&[10, 10, 10]);
    let mut oracle = o(&text);
    let mut map = update(mk(&text, vec![]), &oracle, &mut measured(0, &[12.0, 12.0, 12.0]));
    let new_text = text.replace(21, 21, &Text::of(&["!"]));
    oracle.set_doc(&new_text);
    map = map.apply_changes(&[], &text, &mut oracle, &[changed(21, 21, 21, 22)]);
    clear_height_change_flag();
    let _ = update(map, &oracle, &mut measured(0, &[12.0, 12.0, 12.0]));
    assert!(!height_change_flag());
}

#[test]
fn can_update_lines_across_the_tree() {
    let text = doc(&[10; 100]);
    let oracle = o(&text);
    let mut map = update(mk(&text, vec![]), &oracle, &mut measured(0, &[12.0; 100]));
    assert_eq!(map.height(), 1200.0);
    assert_eq!(map.size(), 100);
    map = update(map, &oracle, &mut measured(55, &[10.0; 90]));
    assert_eq!(map.height(), 1020.0);
    assert_eq!(map.size(), 100);
}

fn depth(map: &HeightMap) -> usize {
    match map {
        HeightMap::Branch(b) => depth(&b.left).max(depth(&b.right)) + 1,
        _ => 1,
    }
}

#[test]
fn balances_a_big_tree() {
    let text = doc(&[30; 100]);
    let mut oracle = o(&text);
    let mut map = update(mk(&text, vec![]), &oracle, &mut measured(0, &[15.0; 100]));
    assert_eq!(map.height(), 1500.0);
    assert_eq!(map.size(), 100);
    assert!(depth(&map) < 9);
    let text2 = text.replace(0, 31 * 80, &Text::empty());
    map = map.apply_changes(&[], &text, &mut o(&text2), &[changed(0, 31 * 80, 0, 0)]);
    assert_eq!(map.size(), 20);
    assert!(depth(&map) < 7);
    let len = text2.len();
    // "\nfoo".repeat(200).split("\n")
    let mut foo = vec![""];
    foo.extend(std::iter::repeat_n("foo", 200));
    let text3 = text2.replace(len, len, &Text::of(&foo));
    map = map.apply_changes(&[], &text2, &mut o(&text3), &[changed(len, len, len, len + 800)]);
    oracle.set_doc(&text3);
    map = update(map, &oracle, &mut measured(len + 1, &[10.0; 200]));
    assert_eq!(map.size(), 220);
    assert!(depth(&map) < 12);
}

#[test]
fn can_handle_inserting_a_line_break() {
    let text = doc(&[3, 3, 3]);
    let mut oracle = o(&text);
    let mut map = update(mk(&text, vec![]), &oracle, &mut measured(0, &[10.0, 10.0, 10.0]));
    assert_eq!(map.size(), 3);
    let text2 = text.replace(3, 3, &Text::of(&["", ""]));
    oracle.set_doc(&text2);
    map = map.apply_changes(&[], &text, &mut oracle, &[changed(3, 3, 3, 4)]);
    map = update(map, &oracle, &mut measured(0, &[10.0, 10.0, 10.0, 10.0]));
    assert_eq!(map.size(), 4);
    assert_eq!(map.height(), 40.0);
}

#[test]
fn can_handle_insertion_in_the_middle_of_a_line() {
    let text = doc(&[3, 3, 3]);
    let oracle = o(&text);
    let mut map = update(mk(&text, vec![]), &oracle, &mut measured(0, &[10.0, 10.0, 10.0]));
    let text2 = text.replace(5, 5, &Text::of(&["foo", "bar", "baz", "bug"]));
    map = map.apply_changes(&[], &text, &mut o(&text2), &[changed(5, 5, 5, 20)]);
    map = update(map, &o(&text2), &mut measured(0, &[10.0; 6]));
    assert_eq!(map.size(), 6);
    assert_eq!(map.height(), 60.0);
}

#[test]
fn properly_shrinks_gaps_when_partially_replaced() {
    let text = doc(&[9, 9, 9, 9, 9, 9, 9]);
    let mut map = update(mk(&text, vec![]), &o(&text), &mut measured(20, &[10.0, 10.0, 10.0]));
    assert_eq!(map.to_string(), "gap(19) line(9) line(9) line(9) gap(19)");
    map = map.apply_changes(
        &[Decoration::set(vec![replace().range(15, 55)], false)],
        &text,
        &mut o(&text),
        &[changed(15, 55, 15, 55)],
    );
    assert_eq!(map.to_string(), "gap(9) line(49-40) gap(9)");
}

#[test]
fn block_at_finds_blocks_in_a_gap() {
    let text = doc(&[3, 3, 3, 3, 3]);
    let map = mk(&text, vec![]);
    let oracle = o(&text);
    let block1 = map.block_at(0.0, &oracle, 0.0, 0);
    assert_eq!((block1.from, block1.to(), block1.top), (0, 3, 0.0));
    assert!(block1.bottom() > 0.0);
    assert_eq!(block1.block_type(), BlockType::Text);
    let block2 = map.block_at(block1.bottom() + 1.0, &oracle, 0.0, 0);
    assert_eq!((block2.from, block2.to(), block2.top), (4, 7, block1.bottom()));
    assert!(block2.bottom() > block1.bottom());
    let block3 = map.block_at(1e9, &oracle, 0.0, 0);
    assert_eq!((block3.from, block3.to(), block3.bottom()), (16, 19, map.height()));
}

#[test]
fn block_at_finds_blocks_in_lines() {
    let text = doc(&[3, 3, 3, 3]);
    let oracle = o(&text);
    let map = update(mk(&text, vec![]), &oracle, &mut measured(0, &[10.0, 20.0, 10.0, 30.0]));
    let block1 = map.block_at(-100.0, &oracle, 0.0, 0);
    assert_eq!((block1.from, block1.to(), block1.top, block1.bottom()), (0, 3, 0.0, 10.0));
    assert_eq!(block1.block_type(), BlockType::Text);
    let block2 = map.block_at(39.0, &oracle, 0.0, 0);
    assert_eq!((block2.from, block2.to(), block2.top, block2.bottom()), (8, 11, 30.0, 40.0));
    let block3 = map.block_at(77.0, &oracle, 0.0, 0);
    assert_eq!((block3.from, block3.to(), block3.top, block3.bottom()), (12, 15, 40.0, 70.0));
}

#[test]
fn block_at_finds_widget_blocks() {
    let text = doc(&[3, 3, 3, 3]);
    let map =
        mk(&text, vec![widget(100.0, -1, true).at(4), replace_block(30.0).range(8, 11), widget(0.0, 1, true).at(15)]);
    let oracle = o(&text);
    let block1 = map.block_at(0.0, &oracle, 0.0, 0);
    assert_eq!((block1.from, block1.to()), (0, 3));
    let block2 = map.block_at(block1.height + 1.0, &oracle, 0.0, 0);
    assert_eq!((block2.from, block2.to(), block2.top, block2.height), (4, 4, block1.height, 100.0));
    assert_eq!(block2.block_type(), BlockType::WidgetBefore);
    let top3 = block2.bottom() + block1.height;
    let block3 = map.block_at(top3 + 10.0, &oracle, 0.0, 0);
    assert_eq!((block3.from, block3.to(), block3.top, block3.height), (8, 11, top3, 30.0));
    assert_eq!(block3.block_type(), BlockType::WidgetRange);
    let block4 = map.block_at(block3.bottom() + block1.height, &oracle, 0.0, 0);
    assert_eq!(block4.block_type(), BlockType::WidgetAfter);
}

#[test]
fn line_at_finds_lines_in_gaps() {
    let text = doc(&[3, 3, 3, 3]);
    let map = mk(&text, vec![]);
    let oracle = o(&text);
    let line1 = by_p(&map, 0, &oracle);
    assert_eq!((line1.from, line1.to(), line1.top), (0, 3, 0.0));
    assert!(eq_block(&by_h(&map, 0.0, &oracle), &line1));
    let line2 = by_p(&map, line1.to() + 1, &oracle);
    assert_eq!((line2.from, line2.to(), line2.top), (4, 7, line1.bottom()));
    assert!(eq_block(&by_h(&map, line1.bottom() + 1.0, &oracle), &line2));
    let line3 = by_p(&map, 15, &oracle);
    assert_eq!((line3.from, line3.to(), line3.bottom()), (12, 15, map.height()));
    assert!(eq_block(&by_h(&map, 1e9, &oracle), &line3));
}

#[test]
fn line_at_finds_lines_in_lines() {
    let text = doc(&[3, 3, 3, 3]);
    let oracle = o(&text);
    let map = update(mk(&text, vec![]), &oracle, &mut measured(0, &[10.0, 10.0, 20.0, 10.0]));
    let line1 = by_p(&map, 0, &oracle);
    assert_eq!((line1.from, line1.to(), line1.top, line1.bottom()), (0, 3, 0.0, 10.0));
    assert!(eq_block(&by_h(&map, 9.0, &oracle), &line1));
    let line2 = by_p(&map, 9, &oracle);
    assert_eq!((line2.from, line2.to(), line2.top, line2.bottom()), (8, 11, 20.0, 40.0));
    assert!(eq_block(&by_h(&map, 39.0, &oracle), &line2));
}

#[test]
fn line_at_includes_adjacent_widgets_in_lines() {
    let text = doc(&[3, 3, 3, 3]);
    let oracle = o(&text);
    let map =
        mk(&text, vec![widget(100.0, -1, true).at(4), replace_block(30.0).range(7, 8), widget(0.0, 1, true).at(15)]);
    let line1 = by_p(&map, 4, &oracle);
    assert_eq!((line1.from, line1.to()), (4, 11));
    assert_eq!(line1.children().unwrap().len(), 4);
    for probe in [
        by_h(&map, line1.top + 1.0, &oracle),
        by_h(&map, line1.bottom() - 1.0, &oracle),
        by_h(&map, line1.top + line1.height / 2.0, &oracle),
        by_p(&map, 5, &oracle),
        by_p(&map, 7, &oracle),
        by_p(&map, 11, &oracle),
    ] {
        assert!(eq_block(&probe, &line1));
    }
    let line2 = by_h(&map, map.height(), &oracle);
    assert_eq!((line2.from, line2.to()), (12, 15));
    assert_eq!(line2.children().unwrap().len(), 2);
    assert!(eq_block(&by_h(&map, line2.top + 1.0, &oracle), &line2));
}

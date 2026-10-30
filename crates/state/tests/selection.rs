//! Port of CodeMirror's `test/test-selection.ts`.

use codemirror_state::{EditorSelection, SelectionRange};

fn describe(ranges: &[SelectionRange]) -> String {
    ranges.iter().map(|r| format!("{}/{}", r.anchor(), r.head())).collect::<Vec<_>>().join(",")
}

#[test]
fn stores_ranges_with_a_primary_range() {
    let sel = EditorSelection::create(
        vec![EditorSelection::range(0, 1), EditorSelection::range(3, 2), EditorSelection::range(4, 5)],
        1,
    );
    assert_eq!(sel.main().from, 2);
    assert_eq!(sel.main().to, 3);
    assert_eq!(sel.main().anchor(), 3);
    assert_eq!(sel.main().head(), 2);
    assert_eq!(describe(&sel.ranges), "0/1,3/2,4/5");
}

#[test]
fn merges_and_sorts_ranges_when_normalizing() {
    let sel = EditorSelection::create(
        vec![
            EditorSelection::range(10, 12),
            EditorSelection::range(6, 7),
            EditorSelection::range(4, 5),
            EditorSelection::range(3, 4),
            EditorSelection::range(0, 6),
            EditorSelection::range(7, 8),
            EditorSelection::range(9, 13),
            EditorSelection::range(13, 14),
        ],
        0,
    );
    assert_eq!(describe(&sel.ranges), "0/6,6/7,7/8,9/13,13/14");
}

#[test]
fn merges_adjacent_point_ranges_when_normalizing() {
    let sel = EditorSelection::create(
        vec![
            EditorSelection::range(10, 12),
            EditorSelection::range(12, 12),
            EditorSelection::range(12, 12),
            EditorSelection::range(10, 10),
            EditorSelection::range(8, 10),
        ],
        0,
    );
    assert_eq!(describe(&sel.ranges), "8/10,10/12");
}

#[test]
fn preserves_the_direction_of_the_last_range_when_merging_ranges() {
    let sel = EditorSelection::create(vec![EditorSelection::range(0, 2), EditorSelection::range(10, 1)], 0);
    assert_eq!(describe(&sel.ranges), "10/0");
}

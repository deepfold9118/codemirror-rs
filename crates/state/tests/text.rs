//! Port of CodeMirror's `test/test-text.ts`. Tests about the shape of
//! CodeMirror's own tree (depth, node boundaries) and its bidirectional
//! iterator are not ported; this `Text` wraps a rope.

mod common;

use codemirror_state::Text;
use common::Rng;

fn line() -> String {
    "1234567890".repeat(10)
}

fn lines() -> Vec<String> {
    vec![line(); 200]
}

fn text0() -> String {
    lines().join("\n")
}

fn doc0() -> Text {
    Text::of(&lines())
}

#[test]
fn handles_basic_replacement() {
    let doc = Text::of(&["one", "two", "three"]);
    assert_eq!(doc.replace(2, 5, &Text::of(&["foo", "bar"])).to_string(), "onfoo\nbarwo\nthree");
}

#[test]
fn can_append_documents() {
    let doc = Text::of(&["one", "two", "three"]).append(&Text::of(&["!", "ok"]));
    assert_eq!(doc.to_string(), "one\ntwo\nthree!\nok");
}

#[test]
fn preserves_length() {
    assert_eq!(doc0().len(), text0().len());
}

#[test]
fn rebalances_on_insert() {
    let mut doc = doc0();
    let text0 = text0();
    let insert = "abc".repeat(200);
    let at = doc.len() / 2;
    for _ in 0..10 {
        doc = doc.replace(at, at, &Text::of(&[insert.as_str()]));
    }
    assert_eq!(doc.to_string(), format!("{}{}{}", &text0[..at], "abc".repeat(2000), &text0[at..]));
}

#[test]
fn collapses_on_delete() {
    let doc = doc0().replace(10, text0().len() - 10, &Text::empty());
    assert_eq!(doc.len(), 20);
    assert_eq!(doc.to_string(), line()[..20]);
}

#[test]
fn handles_deleting_at_start() {
    let mut ls = lines();
    ls.pop();
    ls.push(format!("{}!", line()));
    let doc = Text::of(&ls).replace(0, 9500, &Text::empty());
    assert_eq!(doc.to_string(), format!("{}!", &text0()[9500..]));
}

#[test]
fn handles_deleting_at_end() {
    let mut ls = vec![format!("?{}", line())];
    ls.extend(lines().into_iter().skip(1));
    let doc = Text::of(&ls).replace(9500, text0().len() + 1, &Text::empty());
    assert_eq!(doc.to_string(), format!("?{}", &text0()[..9499]));
}

#[test]
fn can_handle_deleting_the_entire_document() {
    let doc = doc0();
    assert_eq!(doc.replace(0, doc.len(), &Text::empty()).to_string(), "");
}

#[test]
fn can_build_up_a_doc_by_repeated_appending() {
    let mut doc = Text::of(&[""]);
    let mut text = String::new();
    for i in 1..1000 {
        let add = format!("newtext{i} ");
        doc = doc.replace(doc.len(), doc.len(), &Text::of(&[add.as_str()]));
        text += &add;
    }
    assert_eq!(doc.to_string(), text);
}

#[test]
fn properly_maintains_content_during_editing() {
    let mut rng = Rng::new(7);
    let mut s = text0();
    let mut doc = doc0();
    for _ in 0..200 {
        let ins_pos = rng.below(doc.len());
        let ins_char = ((b'A' + rng.below(26) as u8) as char).to_string();
        s.insert_str(ins_pos, &ins_char);
        doc = doc.replace(ins_pos, ins_pos, &Text::of(&[ins_char.as_str()]));
        let del_from = rng.below(doc.len());
        let del_to = doc.len().min(del_from + rng.below(20));
        s.replace_range(del_from..del_to, "");
        doc = doc.replace(del_from, del_to, &Text::empty());
    }
    assert_eq!(doc.to_string(), s);
}

#[test]
fn returns_the_correct_strings_for_slice() {
    let text: Vec<String> = (0..1000).map(|i| format!("{i:04}")).collect();
    let doc = Text::of(&text);
    let s = text.join("\n");
    let mut rng = Rng::new(8);
    for i in 0..400 {
        let start = if i == 0 { 0 } else { rng.below(doc.len()) };
        let end = if i == 399 { doc.len() } else { start + rng.below(doc.len() - start) };
        assert_eq!(doc.slice(start, end).to_string(), s[start..end]);
    }
}

#[test]
fn can_be_compared() {
    let doc = doc0();
    let doc2 = Text::of(&lines());
    assert!(doc == doc);
    assert!(doc == doc2);
    assert!(doc2 == doc);
    assert!(doc != doc2.replace(5000, 5000, &Text::of(&["y"])));
    assert!(doc != doc2.replace(5000, 5001, &Text::of(&["y"])));
    assert!(doc == doc.replace(5000, 5001, &doc.slice(5000, 5001)));
    assert!(doc != doc.replace(5000, 5001, &Text::of(&["y"])));
}

#[test]
fn can_be_compared_despite_different_tree_shape() {
    let mut ls = vec![format!("{}abc", line())];
    ls.extend(lines().into_iter().skip(2));
    assert!(doc0().replace(100, 201, &Text::of(&["abc"])) == Text::of(&ls));
}

#[test]
fn can_compare_small_documents() {
    assert!(Text::of(&["foo", "bar"]) == Text::of(&["foo", "bar"]));
    assert!(Text::of(&["foo", "bar"]) != Text::of(&["foo", "baz"]));
}

#[test]
fn is_iterable() {
    let built: String = doc0().chunks().collect();
    assert_eq!(built, text0());
}

#[test]
fn is_partially_iterable() {
    let doc = doc0();
    let found: String = doc.iter_range(500, doc.len() - 500).collect();
    assert_eq!(found, text0()[500..text0().len() - 500]);
}

#[test]
fn can_partially_iter_over_subsections_at_the_start_and_end() {
    let doc = doc0();
    assert_eq!(doc.iter_range(0, 1).next(), Some("1"));
    assert_eq!(doc.iter_range(1, 2).next(), Some("2"));
    assert_eq!(doc.iter_range(doc.len() - 1, doc.len()).next(), Some("0"));
    assert_eq!(doc.iter_range(doc.len() - 2, doc.len() - 1).next(), Some("9"));
}

#[test]
fn can_iterate_over_lines() {
    let doc = Text::of(&["ab", "cde", "", "", "f", "", "g"]);
    let get = |from: Option<usize>, to: Option<usize>| doc.iter_lines(from, to).collect::<Vec<_>>().join("\n");
    assert_eq!(get(None, None), "ab\ncde\n\n\nf\n\ng");
    assert_eq!(get(Some(1), Some(doc.lines() + 1)), "ab\ncde\n\n\nf\n\ng");
    assert_eq!(get(Some(2), Some(3)), "cde");
    assert_eq!(get(Some(1), Some(5)), "ab\ncde\n\n");
    assert_eq!(get(Some(2), Some(1)), "");
    assert_eq!(get(Some(3), None), "\n\nf\n\ng");
}

#[test]
fn can_convert_to_lines() {
    let mut ls = lines();
    ls.extend((0..200).map(|i| format!("line {i}")));
    let text = Text::of(&ls);
    assert!(Text::of(&text.to_lines()) == text);
}

#[test]
fn can_get_line_info_by_line_number() {
    let doc = doc0();
    assert!(std::panic::catch_unwind(|| doc0().line(0)).is_err());
    assert!(std::panic::catch_unwind(|| {
        let d = doc0();
        d.line(d.lines() + 1)
    })
    .is_err());
    for i in (1..doc.lines()).step_by(5) {
        let l = doc.line(i);
        assert_eq!(l.from, (i - 1) * 101);
        assert_eq!(l.to, i * 101 - 1);
        assert_eq!(l.number, i);
        assert_eq!(l.text, line());
    }
}

#[test]
fn can_get_line_info_by_position() {
    let doc = doc0();
    assert!(std::panic::catch_unwind(|| {
        let d = doc0();
        d.line_at(d.len() + 1)
    })
    .is_err());
    for i in (0..doc.len()).step_by(5) {
        let l = doc.line_at(i);
        assert_eq!(l.from, i - (i % 101));
        assert_eq!(l.to, i - (i % 101) + 100);
        assert_eq!(l.number, i / 101 + 1);
        assert_eq!(l.text, line());
    }
}

#[test]
fn can_delete_a_range_at_the_start_of_a_child_node() {
    assert_eq!(doc0().replace(0, 100, &Text::of(&["x"])).to_string(), format!("x{}", &text0()[100..]));
}

#[test]
fn can_retrieve_pieces_of_text() {
    let doc = doc0();
    let text0 = text0();
    let mut rng = Rng::new(9);
    for _ in 0..500 {
        let from = rng.below(doc.len() - 1);
        let to = if rng.below(2) == 0 { from + 2 } else { from + rng.below(doc.len() - 1 - from) + 1 };
        assert_eq!(doc.slice_string(from, to, "\n"), text0[from..to]);
        assert_eq!(doc.slice(from, to).to_string(), text0[from..to]);
    }
}

#[test]
fn clips_out_of_range_boundaries() {
    assert_eq!(Text::empty().slice(0, 10).len(), 0);
    assert_eq!(Text::empty().slice(1000, 1100).len(), 0);
    assert_eq!(doc0().slice(5, 0).len(), 0);
}

#[test]
fn counts_lines_like_codemirror() {
    // Not in CodeMirror's suite: a trailing line break starts an empty line.
    assert_eq!(Text::empty().lines(), 1);
    assert_eq!(Text::from("a\n").lines(), 2);
    assert_eq!(Text::from("a\n").line(2).from, 2);
    assert_eq!(Text::from("a\r\nb\rc").to_lines(), vec!["a", "b", "c"]);
    assert_eq!(Text::from("a\nb").slice_string(0, 3, "\r\n"), "a\r\nb");
}

#[test]
fn line_at_accepts_a_position_inside_a_character() {
    // "─" is three bytes; byte 5 is inside the first one on line 2.
    let doc = Text::of(&["abc", "──x"]);
    let line = doc.line_at(5);
    assert_eq!((line.number, line.from, line.to), (2, 4, 11));
}

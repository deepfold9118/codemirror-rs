# codemirror-rs

A Rust port of [CodeMirror 6](https://codemirror.net/), package by package,
with the view layer drawn by [GPUI](https://gpui.rs), Zed's UI framework,
instead of the DOM. It is being built as the editing core for
[Notey](https://github.com/deepfold9118/notey).

Ported from the sources on Marijn Haverbeke's forge,
`code.haverbeke.berlin` (October 2026: `@codemirror/state` 6.7.6,
`@codemirror/commands` 6.11.1, `@codemirror/language` 6.13.1,
`@codemirror/view` 6.43.15).

## Status

| Crate | CodeMirror package | State |
|---|---|---|
| `codemirror-state` | `@codemirror/state` | **Ported.** `Text`, `ChangeSet`/`ChangeDesc`, `EditorSelection`, `Transaction`, annotations, state effects, `Facet`, `StateField`, `Prec`, `Compartment`, change/transaction filters and extenders, `RangeSet`, char categories, columns, grapheme clusters. CodeMirror's test suites for text, changes, selections, facets, state and range sets are ported and pass (170 tests). |
| `codemirror-commands` | `@codemirror/commands` | **Ported, except the syntax-tree commands.** Undo history (grouping, isolation, selection undo, inverted effects, remote changes), comment toggling, editing commands (delete by character, group, indent unit or line; newline with indentation; Tab; indent/dedent; move/copy lines; transpose; trailing whitespace), selection commands, matching bracket, and the view commands: motion by character, group, subword, line, page and line boundary, add cursor above/below. Keymaps: `standard_keymap`, `default_keymap`, `emacs_style_keymap`, `history_keymap`, `indent_with_tab`. CodeMirror's history, comment and command tests are ported and pass, except those that need a parser (JavaScript/HTML) or widgets; the browser tests for vertical and subword motion run in GPUI's test context, with one more test that types and presses keys through the view (99 tests). `cursorSyntax*`, `selectParentSyntax` and tab-focus mode wait for the language package. |
| `codemirror-view` | `@codemirror/view` | **First slice, drawn with GPUI** (`gpui` 0.2.2). `EditorView` is a GPUI entity: virtualised lines, word wrap, tab stops, multiple selections and carets, active line, line numbers, focus, IME composition through GPUI's input handler, a scrollbar, scroll anchoring while heights are measured, clipboard with CodeMirror's line-wise copy and paste. Mouse selection: click, double/triple click, drag, Shift-click, Ctrl-click to add or remove a cursor, Alt-drag rectangles, autoscroll. View plugins, `ViewUpdate`, update listeners, decorations (marks with colors, underlines, strikethrough, italic and bold; line backgrounds), the height map (ported with its tests, 26 with the keymap tests), key bindings with multi-stroke prefixes, cursor motion and coordinate queries, scroll into view. Not yet: drawing widgets and replaced (folded) ranges, bidirectional text, atomic ranges, panels, tooltips, a general gutter API, and drag and drop. |
| `codemirror-language` | `@codemirror/language` | **Parser-free slice:** `INDENT_UNIT`, `INDENT_SERVICE`, `IndentContext`, `get_indentation`, `indent_range`, `indent_string`, and plain-text `match_brackets`. These behave like CodeMirror with no language loaded. The parser (Lezer port, tree-sitter or a syntect adapter) is still to be decided. |
| `codemirror-search` | `@codemirror/search` | Planned. |
| `codemirror-autocomplete` | `@codemirror/autocomplete` | Planned. |

## Differences from CodeMirror

- **Positions are UTF-8 byte offsets**, not UTF-16 code units. Every
  position lands on a char boundary.
- **The document is a rope** (`crop`), wrapped in `Text` with CodeMirror's
  API (`line`, `line_at`, `slice`, `replace`, ...). Lines are always
  separated by `"\n"` internally. A configured `LINE_SEPARATOR` is used to
  split input and join output, but a lone `"\n"` is always a line break
  (CodeMirror keeps it inside a line when the separator is `"\r\n"`).
- **Typed handles over erased values.** Facets, fields, annotation types and
  effect types are handles (`Facet<I, O>`, `StateField<V>`, ...) over values
  stored as `Arc<dyn Any>`. Define them once, usually in a `static`
  `LazyLock`, and clone the handle; clones keep their identity.
- **Identity.** Where CodeMirror compares objects with `===`, this port
  compares `Arc` pointers. Facets and fields therefore treat a recomputed
  value as changed unless you supply a comparison (`compare_eq`,
  `compare_input_eq`, or a custom function).
- **Errors panic.** Out-of-range positions and mismatched change sets panic
  where CodeMirror throws `RangeError`.
- **Commands are plain functions**: `StateCommand` is
  `fn(&EditorState, &mut dyn FnMut(Transaction)) -> bool`, where CodeMirror
  passes a `{state, dispatch}` object.
- `can_redo_selection` checks the redo branch; CodeMirror's checks the undo
  branch, which looks like a slip.
- `match_brackets` counts its scan distance in bytes, one character at a
  time, rather than in UTF-16 units per text chunk.
- **The view is a GPUI element.** Instead of keeping a DOM in sync, it lays
  out the lines in view each frame (GPUI caches line layouts between
  frames) and feeds their heights to the height map. Marks carry a
  `MarkStyle` (GPUI text run styles) where CodeMirror uses CSS classes, and
  the theme is a `Theme` value in the `THEME` facet. View commands are
  `fn(&mut EditorView, &mut Window, &mut Context<EditorView>) -> bool`, since
  motion needs the window for text layout; key bindings take either kind of
  command. The clipboard keys are handled by the view, as a browser does
  for CodeMirror.
- Text is treated as left-to-right: motion is logical, by grapheme cluster.
- Not ported yet: JSON serialization of states and change sets, the
  bidirectional/skip-capable `Text` iterator (chunks and line iterators are
  available), and `findClusterBreak`'s joining of zero-width-joiner
  sequences when `include_extending` is false.

## Building

```bash
cargo test
```

To try the editor in a window:

```bash
cargo run --release -p codemirror-demo
```

Debug builds use `opt-level = 1` (and 2 for dependencies): fully
unoptimized, the editor is 20 to 40 times slower.

## License

MIT, like CodeMirror. See [LICENSE](LICENSE): the port keeps CodeMirror's
copyright notice.

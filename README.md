# codemirror-rs

A Rust port of [CodeMirror 6](https://codemirror.net/), package by package,
with the view layer rendered by [egui](https://github.com/emilk/egui)
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
| `codemirror-commands` | `@codemirror/commands` | **Ported, except the syntax-tree commands.** Undo history (grouping, isolation, selection undo, inverted effects, remote changes), comment toggling, editing commands (delete by character, group, indent unit or line; newline with indentation; Tab; indent/dedent; move/copy lines; transpose; trailing whitespace), selection commands, matching bracket, and the view commands: motion by character, group, subword, line, page and line boundary, add cursor above/below. Keymaps: `standard_keymap`, `default_keymap`, `emacs_style_keymap`, `history_keymap`, `indent_with_tab`. CodeMirror's history, comment and command tests are ported and pass, except those that need a parser (JavaScript/HTML) or widgets; the browser tests for vertical and subword motion run against a headless egui context (99 tests). `cursorSyntax*`, `selectParentSyntax` and tab-focus mode wait for the language package. |
| `codemirror-view` | `@codemirror/view` | **First slice, drawn with egui.** `EditorView` with `ui()`: virtualised lines, word wrap, tab stops, multiple selections and carets, active line, line numbers, focus, IME composition and candidate-window position, clipboard with CodeMirror's line-wise copy and paste. Mouse selection: click, double/triple click, drag, Shift-click, Ctrl-click to add or remove a cursor, Alt-drag rectangles, autoscroll. View plugins, `ViewUpdate`, update listeners, decorations (marks with egui text styles, line backgrounds), the height map (ported with its tests), key bindings with multi-stroke prefixes, cursor motion and coordinate queries, scroll into view. Not yet: drawing widgets and replaced (folded) ranges, bidirectional text, atomic ranges, panels, tooltips, a general gutter API, drag and drop, and documents taller than about 16 million points. |
| `codemirror-gpui` | `@codemirror/view` (GPUI) | **Spike.** The same state and commands in a view drawn with [GPUI](https://gpui.rs) 0.2.2 (Zed's UI framework): virtualised lines, wrap, tab stops, multiple selections and carets, active line, line numbers, scrollbar, IME through GPUI's input handler, clipboard, mouse selection (drag, double/triple click, Ctrl-click, Alt-drag rectangles), and the default key bindings. No decorations, view plugins or CodeMirror height map yet (a simpler line-height index stands in). |
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
- **The view is immediate-mode.** Instead of keeping a DOM in sync,
  `EditorView::ui` lays out the lines in view with egui each frame and
  feeds their heights to the height map. Marks carry a `MarkStyle` (an egui
  text format) where CodeMirror uses CSS classes, and the theme is a
  `Theme` value in the `THEME` facet. View commands are
  `fn(&mut EditorView) -> bool`; key bindings take either kind of command.
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

The GPUI version:

```bash
cargo run --release -p codemirror-gpui --example editor
```

Use `--release`: unoptimized builds of the views are 20–40 times slower.

## License

MIT, like CodeMirror. See [LICENSE](LICENSE): the port keeps CodeMirror's
copyright notice.

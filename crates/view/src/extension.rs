//! View plugins, view updates and the view's facets. Port of
//! `extension.ts`.

use std::any::Any;
use std::marker::PhantomData;
use std::sync::{Arc, LazyLock};

use codemirror_state::{
    ChangeDesc, ChangeSet, EditorState, Extension, Facet, FacetSpec, SelectionRange, StateCommand, StateEffectType,
    Transaction,
};

use crate::decoration::{Decoration, DecorationSet};
use crate::editorview::EditorView;
use crate::theme::Theme;
use crate::viewstate::Viewport;
use gpui::{Context, Window};

/// Command functions are used in key bindings and other types of user
/// actions. Given an editor view, they check whether their effect can
/// apply to the editor, and if it can, perform it as a side effect (which
/// usually means dispatching a transaction) and return `true`. They get the
/// GPUI window, which motion commands need for text layout.
pub type Command = fn(&mut EditorView, &mut Window, &mut Context<EditorView>) -> bool;

/// A command that a key binding runs: either a view [`Command`] or a
/// [`StateCommand`], which the view runs against its own state.
#[derive(Clone, Copy)]
pub enum Cmd {
    View(Command),
    State(StateCommand),
}

impl Cmd {
    /// Run the command on the view.
    pub fn run(self, view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
        match self {
            Cmd::View(f) => f(view, window, cx),
            Cmd::State(f) => view.run_state_command(f, cx),
        }
    }

    pub(crate) fn same(self, other: Cmd) -> bool {
        match (self, other) {
            (Cmd::View(a), Cmd::View(b)) => std::ptr::fn_addr_eq(a, b),
            (Cmd::State(a), Cmd::State(b)) => std::ptr::fn_addr_eq(a, b),
            _ => false,
        }
    }
}

/// Functions that are called after every view update.
pub type UpdateListener = Arc<dyn Fn(&ViewUpdate) + Send + Sync>;

/// Facet to register update listeners.
pub static UPDATE_LISTENER: LazyLock<Facet<UpdateListener>> = LazyLock::new(Facet::list);

/// Facet that controls whether the editor content is editable. When its
/// highest-precedence value is `false`, the editor does not take text input
/// or run editing commands from input. Defaults to true.
pub static EDITABLE: LazyLock<Facet<bool, bool>> = LazyLock::new(|| {
    Facet::define(FacetSpec::combine(|values: &[bool]| values.first().copied().unwrap_or(true)).compare_eq())
});

/// When enabled, lines wrap at the width of the editor.
pub static LINE_WRAPPING: LazyLock<Facet<bool, bool>> =
    LazyLock::new(|| Facet::define(FacetSpec::combine(|values: &[bool]| values.iter().any(|v| *v)).compare_eq()));

/// The visual theme. The first (highest-precedence) value is used; without
/// one, [`Theme::dark`] is used.
pub static THEME: LazyLock<Facet<Arc<Theme>, Option<Arc<Theme>>>> =
    LazyLock::new(|| Facet::define(FacetSpec::combine(|values: &[Arc<Theme>]| values.first().cloned())));

/// A source of decorations: a fixed set, or a function of the view (for
/// example one that reads a view plugin).
#[derive(Clone)]
pub enum DecorationSource {
    Set(DecorationSet),
    Dynamic(Arc<dyn Fn(&EditorView) -> DecorationSet + Send + Sync>),
}

/// Facet that provides decorations. Higher-precedence sources style the
/// text over lower-precedence ones.
pub static DECORATIONS: LazyLock<Facet<DecorationSource>> = LazyLock::new(Facet::list);

/// A target to scroll into view.
#[derive(Clone, Debug)]
pub struct ScrollTarget {
    pub range: SelectionRange,
    pub y: ScrollStrategy,
    pub x: ScrollStrategy,
    pub y_margin: f32,
    pub x_margin: f32,
}

/// Where to put a [scroll target](ScrollTarget) in the visible area.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollStrategy {
    /// Scroll as little as possible.
    Nearest,
    Start,
    End,
    Center,
}

impl ScrollTarget {
    pub(crate) fn map(&self, changes: &ChangeDesc) -> ScrollTarget {
        ScrollTarget { range: self.range.map(changes, -1), ..self.clone() }
    }
}

/// Effect that makes the view scroll a position into view.
pub static SCROLL_INTO_VIEW: LazyLock<StateEffectType<ScrollTarget>> =
    LazyLock::new(|| StateEffectType::define_with_map(|t: &ScrollTarget, ch| Some(t.map(ch))));

/// Create a [`SCROLL_INTO_VIEW`] target for a position or range.
pub fn scroll_target(range: SelectionRange) -> ScrollTarget {
    ScrollTarget { range, y: ScrollStrategy::Nearest, x: ScrollStrategy::Nearest, y_margin: 5.0, x_margin: 5.0 }
}

/// The value of a view plugin. Plugins are notified of view updates.
pub trait PluginValue: Any {
    /// Notifies the plugin of an update that happened in the view.
    fn update(&mut self, _update: &ViewUpdate) {}

    /// Called when the plugin is no longer going to be used.
    fn destroy(&mut self) {}
}

#[derive(Clone)]
pub(crate) struct PluginSource {
    pub id: u64,
    pub create: Arc<dyn Fn(&EditorView) -> Box<dyn PluginValue> + Send + Sync>,
}

pub(crate) static VIEW_PLUGIN: LazyLock<Facet<PluginSource, Vec<PluginSource>>> = LazyLock::new(|| {
    Facet::define(FacetSpec::combine(|plugins: &[PluginSource]| {
        let mut out: Vec<PluginSource> = Vec::new();
        for p in plugins {
            if !out.iter().any(|q| q.id == p.id) {
                out.push(p.clone());
            }
        }
        out
    }))
});

fn next_plugin_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// View plugins associate stateful values with a view. They are notified
/// of view updates and can provide decorations.
pub struct ViewPlugin<V> {
    pub(crate) id: u64,
    create: Arc<dyn Fn(&EditorView) -> Box<dyn PluginValue> + Send + Sync>,
    extension: Extension,
    _value: PhantomData<fn() -> V>,
}

impl<V> Clone for ViewPlugin<V> {
    fn clone(&self) -> Self {
        ViewPlugin { id: self.id, create: self.create.clone(), extension: self.extension.clone(), _value: PhantomData }
    }
}

impl<V: PluginValue> ViewPlugin<V> {
    /// Define a plugin from a function that creates the plugin's value,
    /// given an editor view.
    pub fn define(create: impl Fn(&EditorView) -> V + Send + Sync + 'static) -> ViewPlugin<V> {
        let id = next_plugin_id();
        let create: Arc<dyn Fn(&EditorView) -> Box<dyn PluginValue> + Send + Sync> =
            Arc::new(move |view| Box::new(create(view)));
        let extension = VIEW_PLUGIN.of(PluginSource { id, create: create.clone() });
        ViewPlugin { id, create, extension, _value: PhantomData }
    }

    /// Let the plugin provide decorations, computed from its value.
    pub fn decorations(self, deco: impl Fn(&V) -> DecorationSet + Send + Sync + 'static) -> ViewPlugin<V> {
        let id = self.id;
        let source = DecorationSource::Dynamic(Arc::new(move |view: &EditorView| {
            view.plugin_by_id::<V>(id).map(&deco).unwrap_or_else(Decoration::none)
        }));
        let extension = Extension::list([self.extension, DECORATIONS.of(source)]);
        ViewPlugin { extension, ..self }
    }

    /// Specify that the plugin provides additional extensions.
    pub fn provide(self, ext: impl Into<Extension>) -> ViewPlugin<V> {
        let extension = Extension::list([self.extension, ext.into()]);
        ViewPlugin { extension, ..self }
    }

    /// The extension that adds this plugin to an editor.
    pub fn extension(&self) -> Extension {
        self.extension.clone()
    }
}

impl<V: PluginValue> From<&ViewPlugin<V>> for Extension {
    fn from(plugin: &ViewPlugin<V>) -> Extension {
        plugin.extension()
    }
}

pub(crate) struct PluginInstance {
    pub id: u64,
    pub value: Box<dyn PluginValue>,
}

pub(crate) mod update_flag {
    pub const FOCUS: u8 = 1;
    pub const HEIGHT: u8 = 2;
    pub const VIEWPORT: u8 = 4;
    pub const VIEWPORT_MOVED: u8 = 8;
    pub const GEOMETRY: u8 = 16;
}

/// A changed range, in both the old (A) and new (B) document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChangedRange {
    pub from_a: usize,
    pub to_a: usize,
    pub from_b: usize,
    pub to_b: usize,
}

impl ChangedRange {
    pub fn new(from_a: usize, to_a: usize, from_b: usize, to_b: usize) -> ChangedRange {
        ChangedRange { from_a, to_a, from_b, to_b }
    }

    // Extend a set to cover all the content in `ranges`, a flat array with
    // each pair of numbers representing from_b/to_b positions. These pairs
    // are generated in unchanged ranges, so the offset between doc A and doc
    // B is the same for their start and end points.
    pub(crate) fn extend_with_ranges(diff: &[ChangedRange], ranges: &[usize]) -> Vec<ChangedRange> {
        if ranges.is_empty() {
            return diff.to_vec();
        }
        const FAR: i64 = 1_000_000_000_000;
        let mut result = Vec::new();
        let (mut d_i, mut r_i, mut off) = (0usize, 0usize, 0i64);
        loop {
            let next_d = if d_i < diff.len() { diff[d_i].from_b as i64 } else { FAR };
            let next_r = if r_i < ranges.len() { ranges[r_i] as i64 } else { FAR };
            let from_b = next_d.min(next_r);
            if from_b == FAR {
                break;
            }
            let from_a = from_b + off;
            let (mut to_b, mut to_a) = (from_b, from_a);
            loop {
                if r_i < ranges.len() && ranges[r_i] as i64 <= to_b {
                    let end = ranges[r_i + 1] as i64;
                    r_i += 2;
                    to_b = to_b.max(end);
                    let mut i = d_i;
                    while i < diff.len() && diff[i].from_b as i64 <= to_b {
                        off = diff[i].to_a as i64 - diff[i].to_b as i64;
                        i += 1;
                    }
                    to_a = to_a.max(end + off);
                } else if d_i < diff.len() && diff[d_i].from_b as i64 <= to_b {
                    let next = diff[d_i];
                    d_i += 1;
                    to_b = to_b.max(next.to_b as i64);
                    to_a = to_a.max(next.to_a as i64);
                    off = next.to_a as i64 - next.to_b as i64;
                } else {
                    break;
                }
            }
            result.push(ChangedRange::new(from_a as usize, to_a as usize, from_b as usize, to_b as usize));
        }
        result
    }
}

/// View plugins and update listeners are given instances of this type,
/// which describe what happened, whenever the view is updated.
pub struct ViewUpdate {
    /// The new editor state.
    pub state: EditorState,
    /// The previous editor state.
    pub start_state: EditorState,
    /// The transactions involved in the update. May be empty.
    pub transactions: Vec<Transaction>,
    /// The changes made to the document by this update.
    pub changes: ChangeSet,
    /// The view's viewport after the update (CodeMirror reads it from
    /// `update.view.viewport`).
    pub viewport: Viewport,
    pub(crate) changed_ranges: Vec<ChangedRange>,
    pub(crate) flags: u8,
}

impl ViewUpdate {
    pub(crate) fn new(start_state: EditorState, state: EditorState, transactions: Vec<Transaction>) -> ViewUpdate {
        let mut changes = ChangeSet::empty(start_state.doc().len());
        for tr in &transactions {
            changes = changes.compose(tr.changes());
        }
        let mut changed_ranges = Vec::new();
        changes.iter_changed_ranges(
            |from_a, to_a, from_b, to_b| changed_ranges.push(ChangedRange::new(from_a, to_a, from_b, to_b)),
            false,
        );
        ViewUpdate { state, start_state, transactions, changes, viewport: Viewport::default(), changed_ranges, flags: 0 }
    }

    /// Whether the viewport changed in this update.
    pub fn viewport_changed(&self) -> bool {
        self.flags & update_flag::VIEWPORT > 0
    }

    /// Whether the viewport changed for a reason other than mapping it
    /// through document changes.
    pub fn viewport_moved(&self) -> bool {
        self.flags & update_flag::VIEWPORT_MOVED > 0
    }

    /// Whether the height of a block element in the editor changed.
    pub fn height_changed(&self) -> bool {
        self.flags & update_flag::HEIGHT > 0
    }

    /// Whether the document was modified or the size of the editor, or
    /// elements within the editor, changed.
    pub fn geometry_changed(&self) -> bool {
        self.doc_changed() || self.flags & (update_flag::GEOMETRY | update_flag::HEIGHT) > 0
    }

    /// Whether this update indicates a focus change.
    pub fn focus_changed(&self) -> bool {
        self.flags & update_flag::FOCUS > 0
    }

    /// Whether the document changed in this update.
    pub fn doc_changed(&self) -> bool {
        !self.changes.is_empty()
    }

    /// Whether the selection was explicitly set in this update.
    pub fn selection_set(&self) -> bool {
        self.transactions.iter().any(|tr| tr.selection().is_some())
    }

    /// Whether nothing happened in this update.
    pub fn is_empty(&self) -> bool {
        self.flags == 0 && self.transactions.is_empty()
    }
}

//! Facets, state fields, extensions and configuration resolution. Port of
//! `facet.ts`.
//!
//! JavaScript's dynamic typing becomes type erasure here: facet inputs and
//! outputs and field values are stored as `Arc<dyn Any>` inside a state, and
//! the typed handles ([`Facet`], [`StateField`]) downcast them on access.
//! Object identity, which CodeMirror uses to detect unchanged values, is
//! `Arc` pointer identity.

use std::any::Any;
use std::collections::{BTreeMap, HashMap};
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, OnceLock};

use crate::state::EditorState;
use crate::transaction::{StateEffect, Transaction, COMPARTMENT_RECONFIGURE};

/// A type-erased value stored in a state.
pub type AnyValue = Arc<dyn Any + Send + Sync>;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

type CompareFn<T> = Arc<dyn Fn(&T, &T) -> bool + Send + Sync>;

// ---------- dependencies ----------

/// A part of the state that a computed facet value depends on. See
/// [`Facet::compute`].
#[derive(Clone)]
pub struct Dep(pub(crate) DepKind);

#[derive(Clone)]
pub(crate) enum DepKind {
    Doc,
    Selection,
    Facet(Arc<dyn ErasedFacet>),
    Field(u64),
}

impl Dep {
    /// Depend on the document.
    pub fn doc() -> Dep {
        Dep(DepKind::Doc)
    }

    /// Depend on the selection (and the document, since document changes
    /// map the selection).
    pub fn selection() -> Dep {
        Dep(DepKind::Selection)
    }
}

// ---------- facets ----------

pub(crate) trait ErasedFacet: Send + Sync {
    fn id(&self) -> u64;
    fn default_value(&self) -> &AnyValue;
    /// Combine provider values; the flag marks multi-value providers.
    fn combine(&self, parts: &[(&AnyValue, bool)]) -> AnyValue;
    fn compare(&self, a: &AnyValue, b: &AnyValue) -> bool;
    fn compare_input(&self, a: &AnyValue, b: &AnyValue, multi: bool) -> bool;
    fn extensions(&self) -> Option<Extension>;
}

struct FacetInner<I, O> {
    id: u64,
    combine: Box<dyn Fn(&[I]) -> O + Send + Sync>,
    compare: CompareFn<O>,
    compare_input: CompareFn<I>,
    is_static: bool,
    default: AnyValue,
    extensions: OnceLock<Option<Extension>>,
}

impl<I, O> ErasedFacet for FacetInner<I, O>
where
    I: Clone + Send + Sync + 'static,
    O: Send + Sync + 'static,
{
    fn id(&self) -> u64 {
        self.id
    }

    fn default_value(&self) -> &AnyValue {
        &self.default
    }

    fn combine(&self, parts: &[(&AnyValue, bool)]) -> AnyValue {
        let mut values: Vec<I> = Vec::with_capacity(parts.len());
        for (value, multi) in parts {
            if *multi {
                values.extend(value.downcast_ref::<Vec<I>>().expect("facet input type").iter().cloned());
            } else {
                values.push(value.downcast_ref::<I>().expect("facet input type").clone());
            }
        }
        Arc::new((self.combine)(&values))
    }

    fn compare(&self, a: &AnyValue, b: &AnyValue) -> bool {
        Arc::ptr_eq(a, b)
            || (self.compare)(
                a.downcast_ref::<O>().expect("facet output type"),
                b.downcast_ref::<O>().expect("facet output type"),
            )
    }

    fn compare_input(&self, a: &AnyValue, b: &AnyValue, multi: bool) -> bool {
        if Arc::ptr_eq(a, b) {
            return true;
        }
        if multi {
            let (a, b) = (a.downcast_ref::<Vec<I>>().unwrap(), b.downcast_ref::<Vec<I>>().unwrap());
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (self.compare_input)(x, y))
        } else {
            (self.compare_input)(a.downcast_ref::<I>().unwrap(), b.downcast_ref::<I>().unwrap())
        }
    }

    fn extensions(&self) -> Option<Extension> {
        self.extensions.get().cloned().flatten()
    }
}

type MakeSameArray<I, O> = Box<dyn FnOnce(CompareFn<I>) -> CompareFn<O> + Send>;
type Enables<I, O> = Box<dyn FnOnce(&Facet<I, O>) -> Extension + Send>;

/// Configuration for [`Facet::define`].
pub struct FacetSpec<I, O> {
    combine: Box<dyn Fn(&[I]) -> O + Send + Sync>,
    compare: Option<CompareFn<O>>,
    same_array: Option<MakeSameArray<I, O>>,
    compare_input: Option<CompareFn<I>>,
    is_static: bool,
    enables: Option<Enables<I, O>>,
}

impl<I: Clone + Send + Sync + 'static> FacetSpec<I, Vec<I>> {
    /// A facet whose output is the list of its inputs, ordered by
    /// precedence. Output values compare equal when their elements do
    /// (by [`compare_input`](FacetSpec::compare_input)).
    pub fn list() -> Self {
        FacetSpec {
            combine: Box::new(|values: &[I]| values.to_vec()),
            compare: None,
            same_array: Some(Box::new(|compare_input: CompareFn<I>| -> CompareFn<Vec<I>> {
                Arc::new(move |a: &Vec<I>, b: &Vec<I>| {
                    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| compare_input(x, y))
                })
            })),
            compare_input: None,
            is_static: false,
            enables: None,
        }
    }
}

impl<I, O> FacetSpec<I, O> {
    /// A facet that combines its inputs (ordered by precedence) into one
    /// output value. `combine` is called with an empty slice to compute the
    /// default value.
    pub fn combine(combine: impl Fn(&[I]) -> O + Send + Sync + 'static) -> Self {
        FacetSpec {
            combine: Box::new(combine),
            compare: None,
            same_array: None,
            compare_input: None,
            is_static: false,
            enables: None,
        }
    }

    /// How to compare output values to determine whether the value of the
    /// facet changed. By default, values only compare equal when they are
    /// the same allocation (CodeMirror's `===`).
    pub fn compare(mut self, compare: impl Fn(&O, &O) -> bool + Send + Sync + 'static) -> Self {
        self.compare = Some(Arc::new(compare));
        self.same_array = None;
        self
    }

    /// How to compare input values to avoid recomputing the output value
    /// when no inputs changed. By default, inputs never compare equal
    /// unless they are the same allocation.
    pub fn compare_input(mut self, compare: impl Fn(&I, &I) -> bool + Send + Sync + 'static) -> Self {
        self.compare_input = Some(Arc::new(compare));
        self
    }

    /// Forbid dynamic inputs to this facet.
    pub fn static_facet(mut self) -> Self {
        self.is_static = true;
        self
    }

    /// Add these extensions to any state where this facet is provided.
    pub fn enables(mut self, ext: impl Into<Extension>) -> Self {
        let ext = ext.into();
        self.enables = Some(Box::new(move |_| ext));
        self
    }

    /// Like [`enables`](FacetSpec::enables), computing the extensions from
    /// the facet itself.
    pub fn enables_with(mut self, f: impl FnOnce(&Facet<I, O>) -> Extension + Send + 'static) -> Self {
        self.enables = Some(Box::new(f));
        self
    }
}

impl<I, O: PartialEq + 'static> FacetSpec<I, O> {
    /// Compare output values with `==`.
    pub fn compare_eq(self) -> Self {
        self.compare(|a: &O, b: &O| a == b)
    }
}

impl<I: PartialEq + 'static, O> FacetSpec<I, O> {
    /// Compare input values with `==`.
    pub fn compare_input_eq(self) -> Self {
        self.compare_input(|a: &I, b: &I| a == b)
    }
}

/// A facet is a labeled value that is associated with an editor state. It
/// takes inputs from any number of extensions, and combines those into a
/// single output value.
///
/// Define facets once (for example in a `static` [`LazyLock`]) and share
/// the handle; cloning a handle keeps its identity.
pub struct Facet<I, O = Vec<I>> {
    inner: Arc<FacetInner<I, O>>,
}

impl<I, O> Clone for Facet<I, O> {
    fn clone(&self) -> Self {
        Facet { inner: self.inner.clone() }
    }
}

impl<I: Clone + Send + Sync + 'static> Facet<I, Vec<I>> {
    /// Define a facet whose output is the list of its inputs.
    pub fn list() -> Self {
        Facet::define(FacetSpec::list())
    }
}

impl<I, O> Facet<I, O>
where
    I: Clone + Send + Sync + 'static,
    O: Send + Sync + 'static,
{
    /// Define a new facet.
    pub fn define(spec: FacetSpec<I, O>) -> Self {
        let compare_input: CompareFn<I> = spec.compare_input.unwrap_or_else(|| Arc::new(|_: &I, _: &I| false));
        let compare: CompareFn<O> = match (spec.compare, spec.same_array) {
            (Some(compare), _) => compare,
            (None, Some(make)) => make(compare_input.clone()),
            (None, None) => Arc::new(|_: &O, _: &O| false),
        };
        let default: AnyValue = Arc::new((spec.combine)(&[]));
        let facet = Facet {
            inner: Arc::new(FacetInner {
                id: next_id(),
                combine: spec.combine,
                compare,
                compare_input,
                is_static: spec.is_static,
                default,
                extensions: OnceLock::new(),
            }),
        };
        let extensions = spec.enables.map(|f| f(&facet));
        let _ = facet.inner.extensions.set(extensions);
        facet
    }

    pub(crate) fn id(&self) -> u64 {
        self.inner.id
    }

    pub(crate) fn erased(&self) -> Arc<dyn ErasedFacet> {
        self.inner.clone()
    }

    pub(crate) fn default_ref(&self) -> &O {
        self.inner.default.downcast_ref::<O>().unwrap()
    }

    /// Use this facet as a dependency in [`compute`](Facet::compute).
    pub fn dep(&self) -> Dep {
        Dep(DepKind::Facet(self.erased()))
    }

    /// Returns an extension that adds the given value to this facet.
    pub fn of(&self, value: I) -> Extension {
        self.provider(Vec::new(), ProviderValue::Static(Arc::new(value)))
    }

    /// Create an extension that computes a value for the facet from a
    /// state. You must declare the parts of the state that this value
    /// depends on, since `get` is only called again for a new state when
    /// one of those parts changed.
    ///
    /// # Panics
    ///
    /// Panics for a static facet.
    pub fn compute(&self, deps: Vec<Dep>, get: impl Fn(&EditorState) -> I + Send + Sync + 'static) -> Extension {
        assert!(!self.inner.is_static, "Can't compute a static facet");
        self.provider(deps, ProviderValue::Single(Arc::new(move |state| Arc::new(get(state)) as AnyValue)))
    }

    /// Create an extension that computes zero or more values for this facet
    /// from a state.
    ///
    /// # Panics
    ///
    /// Panics for a static facet.
    pub fn compute_n(&self, deps: Vec<Dep>, get: impl Fn(&EditorState) -> Vec<I> + Send + Sync + 'static) -> Extension {
        assert!(!self.inner.is_static, "Can't compute a static facet");
        self.provider(deps, ProviderValue::Multi(Arc::new(move |state| Arc::new(get(state)) as AnyValue)))
    }

    /// Register a facet source with a state field as input; `get` reads
    /// the input from the field's value.
    pub fn from<T: Send + Sync + 'static>(
        &self,
        field: &StateField<T>,
        get: impl Fn(&T) -> I + Send + Sync + 'static,
    ) -> Extension {
        let f = field.clone();
        self.compute(vec![field.dep()], move |state| get(state.field(&f)))
    }

    fn provider(&self, deps: Vec<Dep>, value: ProviderValue) -> Extension {
        Extension(Ext::Provider(Arc::new(FacetProvider { id: next_id(), deps, facet: self.erased(), value })))
    }
}

type Getter = Arc<dyn Fn(&EditorState) -> AnyValue + Send + Sync>;

pub(crate) enum ProviderValue {
    Static(AnyValue),
    Single(Getter),
    Multi(Getter),
}

pub(crate) struct FacetProvider {
    pub(crate) id: u64,
    deps: Vec<Dep>,
    pub(crate) facet: Arc<dyn ErasedFacet>,
    pub(crate) value: ProviderValue,
}

impl FacetProvider {
    fn is_static(&self) -> bool {
        matches!(self.value, ProviderValue::Static(_))
    }

    fn is_multi(&self) -> bool {
        matches!(self.value, ProviderValue::Multi(_))
    }

    fn getter(&self) -> &Getter {
        match &self.value {
            ProviderValue::Single(g) | ProviderValue::Multi(g) => g,
            ProviderValue::Static(_) => unreachable!("static providers have no getter"),
        }
    }
}

// ---------- state fields ----------

pub(crate) trait ErasedField: Send + Sync {
    fn id(&self) -> u64;
    fn create_default(&self, state: &EditorState) -> AnyValue;
    fn update(&self, value: &AnyValue, tr: &Transaction) -> AnyValue;
    fn compare(&self, a: &AnyValue, b: &AnyValue) -> bool;
    fn provides(&self) -> Option<Extension>;
}

type FieldCreate<V> = Box<dyn Fn(&EditorState) -> V + Send + Sync>;
type FieldUpdate<V> = Box<dyn Fn(&V, &Transaction) -> V + Send + Sync>;

struct FieldInner<V> {
    id: u64,
    create: FieldCreate<V>,
    update: FieldUpdate<V>,
    compare: Box<dyn Fn(&V, &V) -> bool + Send + Sync>,
    provides: OnceLock<Option<Extension>>,
}

impl<V: Send + Sync + 'static> ErasedField for FieldInner<V> {
    fn id(&self) -> u64 {
        self.id
    }

    fn create_default(&self, state: &EditorState) -> AnyValue {
        Arc::new((self.create)(state))
    }

    fn update(&self, value: &AnyValue, tr: &Transaction) -> AnyValue {
        Arc::new((self.update)(value.downcast_ref::<V>().expect("field value type"), tr))
    }

    fn compare(&self, a: &AnyValue, b: &AnyValue) -> bool {
        Arc::ptr_eq(a, b) || (self.compare)(a.downcast_ref::<V>().unwrap(), b.downcast_ref::<V>().unwrap())
    }

    fn provides(&self) -> Option<Extension> {
        self.provides.get().cloned().flatten()
    }
}

/// Configuration for [`StateField::define`].
pub struct StateFieldSpec<V> {
    create: FieldCreate<V>,
    update: FieldUpdate<V>,
    compare: Option<Box<dyn Fn(&V, &V) -> bool + Send + Sync>>,
    provide: Option<Box<dyn FnOnce(&StateField<V>) -> Extension + Send>>,
}

impl<V> StateFieldSpec<V> {
    /// `create` makes the initial value for the field when a state is
    /// created; `update` computes a new value from the field's previous
    /// value and a transaction.
    pub fn new(
        create: impl Fn(&EditorState) -> V + Send + Sync + 'static,
        update: impl Fn(&V, &Transaction) -> V + Send + Sync + 'static,
    ) -> Self {
        StateFieldSpec { create: Box::new(create), update: Box::new(update), compare: None, provide: None }
    }

    /// Compare two values of the field, returning `true` when they are the
    /// same. This is used to avoid recomputing facets that depend on the
    /// field when its value did not change. By default, every update
    /// counts as a change.
    pub fn compare(mut self, compare: impl Fn(&V, &V) -> bool + Send + Sync + 'static) -> Self {
        self.compare = Some(Box::new(compare));
        self
    }

    /// Provide extensions based on this field. Usually calls some facet's
    /// [`from`](Facet::from) method.
    pub fn provide(mut self, provide: impl FnOnce(&StateField<V>) -> Extension + Send + 'static) -> Self {
        self.provide = Some(Box::new(provide));
        self
    }
}

impl<V: PartialEq> StateFieldSpec<V> {
    /// Compare values with `==`.
    pub fn compare_eq(self) -> Self {
        self.compare(|a: &V, b: &V| a == b)
    }
}

/// Fields can store additional information in an editor state, and keep it
/// in sync with the rest of the state.
pub struct StateField<V> {
    inner: Arc<FieldInner<V>>,
}

impl<V> Clone for StateField<V> {
    fn clone(&self) -> Self {
        StateField { inner: self.inner.clone() }
    }
}

#[derive(Clone)]
pub(crate) struct FieldInit {
    id: u64,
    field: u64,
    create: Arc<dyn Fn(&EditorState) -> AnyValue + Send + Sync>,
}

pub(crate) static INIT_FIELD: LazyLock<Facet<FieldInit>> =
    LazyLock::new(|| Facet::define(FacetSpec::list().static_facet()));

impl<V: Send + Sync + 'static> StateField<V> {
    /// Define a state field.
    pub fn define(spec: StateFieldSpec<V>) -> StateField<V> {
        let field = StateField {
            inner: Arc::new(FieldInner {
                id: next_id(),
                create: spec.create,
                update: spec.update,
                compare: spec.compare.unwrap_or_else(|| Box::new(|_: &V, _: &V| false)),
                provides: OnceLock::new(),
            }),
        };
        let provides = spec.provide.map(|f| f(&field));
        let _ = field.inner.provides.set(provides);
        field
    }

    pub(crate) fn id(&self) -> u64 {
        self.inner.id
    }

    /// Use this field as a dependency in [`Facet::compute`].
    pub fn dep(&self) -> Dep {
        Dep(DepKind::Field(self.inner.id))
    }

    /// Returns an extension that enables this field and overrides the way
    /// it is initialized.
    pub fn init(&self, create: impl Fn(&EditorState) -> V + Send + Sync + 'static) -> Extension {
        let init = FieldInit {
            id: next_id(),
            field: self.inner.id,
            create: Arc::new(move |state| Arc::new(create(state)) as AnyValue),
        };
        Extension::list([self.into(), INIT_FIELD.of(init)])
    }
}

fn create_field(field: &dyn ErasedField, state: &EditorState) -> AnyValue {
    match state.facet(&INIT_FIELD).iter().find(|i| i.field == field.id()) {
        Some(init) => (init.create)(state),
        None => field.create_default(state),
    }
}

// ---------- extensions ----------

/// Extension values can be provided when creating a state to attach various
/// kinds of configuration and behavior information. They can be facet
/// providers, state fields, precedence or compartment wrappers, or lists of
/// extensions nested arbitrarily deep.
///
/// Cloning an extension keeps its identity: the same extension included
/// twice in a configuration is only used once.
#[derive(Clone)]
pub struct Extension(pub(crate) Ext);

#[derive(Clone)]
pub(crate) enum Ext {
    List(Arc<Vec<Extension>>),
    Provider(Arc<FacetProvider>),
    Field(Arc<dyn ErasedField>),
    Prec(Arc<(Extension, u8)>),
    Compartment(Arc<(Compartment, Extension)>),
}

impl Extension {
    /// An extension that holds the given extensions.
    pub fn list<E: Into<Extension>>(items: impl IntoIterator<Item = E>) -> Extension {
        Extension(Ext::List(Arc::new(items.into_iter().map(Into::into).collect())))
    }

    /// An extension that does nothing.
    pub fn empty() -> Extension {
        Extension::list(Vec::<Extension>::new())
    }

    /// True when both values are the same extension (CodeMirror compares
    /// extensions by identity).
    pub fn ptr_eq(&self, other: &Extension) -> bool {
        self.key() == other.key()
    }

    fn key(&self) -> (u8, usize) {
        match &self.0 {
            Ext::List(l) => (0, Arc::as_ptr(l) as usize),
            Ext::Provider(p) => (1, p.id as usize),
            Ext::Field(f) => (2, f.id() as usize),
            Ext::Prec(p) => (3, Arc::as_ptr(p) as usize),
            Ext::Compartment(c) => (4, Arc::as_ptr(c) as usize),
        }
    }
}

impl Default for Extension {
    fn default() -> Self {
        Extension::empty()
    }
}

impl From<Vec<Extension>> for Extension {
    fn from(list: Vec<Extension>) -> Extension {
        Extension::list(list)
    }
}

impl<const N: usize> From<[Extension; N]> for Extension {
    fn from(list: [Extension; N]) -> Extension {
        Extension::list(list)
    }
}

impl<V: Send + Sync + 'static> From<&StateField<V>> for Extension {
    fn from(field: &StateField<V>) -> Extension {
        Extension(Ext::Field(field.inner.clone()))
    }
}

impl<V: Send + Sync + 'static> From<StateField<V>> for Extension {
    fn from(field: StateField<V>) -> Extension {
        Extension(Ext::Field(field.inner))
    }
}

impl From<&Extension> for Extension {
    fn from(ext: &Extension) -> Extension {
        ext.clone()
    }
}

const PREC_HIGHEST: u8 = 0;
const PREC_HIGH: u8 = 1;
const PREC_DEFAULT: u8 = 2;
const PREC_LOW: u8 = 3;
const PREC_LOWEST: u8 = 4;

/// By default extensions are registered in the order they are found in the
/// flattened form of the nested list that was provided. Individual
/// extension values can be assigned a precedence to override this.
/// Extensions that do not have a precedence set get the precedence of the
/// nearest parent with a precedence, or `default` if there is no such
/// parent. The final ordering of extensions is determined by first sorting
/// by precedence and then by order within each precedence.
pub struct Prec(PhantomData<()>);

impl Prec {
    fn wrap(ext: impl Into<Extension>, prec: u8) -> Extension {
        Extension(Ext::Prec(Arc::new((ext.into(), prec))))
    }

    /// The highest precedence level, for extensions that should end up near
    /// the start of the precedence ordering.
    pub fn highest(ext: impl Into<Extension>) -> Extension {
        Prec::wrap(ext, PREC_HIGHEST)
    }

    /// A higher-than-default precedence.
    pub fn high(ext: impl Into<Extension>) -> Extension {
        Prec::wrap(ext, PREC_HIGH)
    }

    /// The default precedence.
    pub fn default(ext: impl Into<Extension>) -> Extension {
        Prec::wrap(ext, PREC_DEFAULT)
    }

    /// A lower-than-default precedence.
    pub fn low(ext: impl Into<Extension>) -> Extension {
        Prec::wrap(ext, PREC_LOW)
    }

    /// The lowest precedence level.
    pub fn lowest(ext: impl Into<Extension>) -> Extension {
        Prec::wrap(ext, PREC_LOWEST)
    }
}

/// Extension compartments can be used to make a configuration dynamic. By
/// [wrapping](Compartment::of) part of your configuration in a compartment,
/// you can later [replace](Compartment::reconfigure) that part through a
/// transaction.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Compartment {
    id: u64,
}

impl Default for Compartment {
    fn default() -> Self {
        Compartment::new()
    }
}

/// The value of the [`COMPARTMENT_RECONFIGURE`] effect.
#[derive(Clone)]
pub struct CompartmentReconfigure {
    pub compartment: Compartment,
    pub extension: Extension,
}

impl Compartment {
    /// Create a new compartment.
    pub fn new() -> Compartment {
        Compartment { id: next_id() }
    }

    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    /// Create an instance of this compartment to add to your state
    /// configuration.
    pub fn of(&self, ext: impl Into<Extension>) -> Extension {
        Extension(Ext::Compartment(Arc::new((self.clone(), ext.into()))))
    }

    /// Create an effect that reconfigures this compartment.
    pub fn reconfigure(&self, content: impl Into<Extension>) -> StateEffect {
        COMPARTMENT_RECONFIGURE.of(CompartmentReconfigure { compartment: self.clone(), extension: content.into() })
    }

    /// Get the current content of the compartment in the state, or `None`
    /// if it isn't present.
    pub fn get<'a>(&self, state: &'a EditorState) -> Option<&'a Extension> {
        state.config().compartments.get(&self.id)
    }
}

// ---------- configuration ----------

pub(crate) const STATUS_CHANGED: u8 = 1;
pub(crate) const STATUS_COMPUTED: u8 = 2;
pub(crate) const STATUS_COMPUTING: u8 = 4;

pub(crate) trait DynamicSlot: Send + Sync {
    fn create(&self, state: &EditorState) -> (AnyValue, bool);
    /// `prev` is the slot's value in the state the transaction starts from
    /// (or the reconfigured intermediate state).
    fn update(&self, state: &EditorState, tr: &Transaction, prev: &AnyValue) -> (AnyValue, bool);
    fn reconfigure(&self, state: &EditorState, old: &EditorState) -> (AnyValue, bool);
}

pub(crate) struct Configuration {
    pub(crate) base: Extension,
    pub(crate) compartments: HashMap<u64, Extension>,
    pub(crate) dynamic_slots: Vec<Box<dyn DynamicSlot>>,
    /// Slot addresses by facet, field or provider id: `index << 1`, with
    /// the low bit set for static values.
    pub(crate) address: HashMap<u64, usize>,
    pub(crate) static_values: Vec<AnyValue>,
    pub(crate) facets: BTreeMap<u64, Vec<Arc<FacetProvider>>>,
}

enum Flat {
    Field(Arc<dyn ErasedField>),
    Provider(Arc<FacetProvider>),
}

impl Flat {
    fn key(&self) -> (u8, usize) {
        match self {
            Flat::Provider(p) => (1, p.id as usize),
            Flat::Field(f) => (2, f.id() as usize),
        }
    }
}

enum SlotMaker {
    Field(Arc<dyn ErasedField>),
    Provider(Arc<FacetProvider>),
    Facet(Arc<dyn ErasedFacet>, Vec<Arc<FacetProvider>>),
}

impl Configuration {
    pub(crate) fn static_facet<'a, O: Send + Sync + 'static>(
        &'a self,
        facet: &'a Facet<impl Clone + Send + Sync + 'static, O>,
    ) -> &'a O {
        match self.address.get(&facet.id()) {
            Some(&addr) if addr & 1 == 1 => self.static_values[addr >> 1].downcast_ref::<O>().unwrap(),
            _ => facet.default_ref(),
        }
    }

    pub(crate) fn resolve(
        base: Extension,
        compartments: &HashMap<u64, Extension>,
        old: Option<&EditorState>,
    ) -> Configuration {
        let mut fields: Vec<Arc<dyn ErasedField>> = Vec::new();
        let mut facets: BTreeMap<u64, Vec<Arc<FacetProvider>>> = BTreeMap::new();
        let mut new_compartments = HashMap::new();

        for ext in flatten(&base, compartments, &mut new_compartments) {
            match ext {
                Flat::Field(f) => fields.push(f),
                Flat::Provider(p) => facets.entry(p.facet.id()).or_default().push(p),
            }
        }

        let mut address: HashMap<u64, usize> = HashMap::new();
        let mut static_values: Vec<AnyValue> = Vec::new();
        let mut makers: Vec<SlotMaker> = Vec::new();

        for field in fields {
            address.insert(field.id(), makers.len() << 1);
            makers.push(SlotMaker::Field(field));
        }

        let old_facets = old.map(|o| &o.config().facets);
        for (&id, providers) in &facets {
            let facet = providers[0].facet.clone();
            let old_providers: &[Arc<FacetProvider>] =
                old_facets.and_then(|f| f.get(&id)).map(|v| v.as_slice()).unwrap_or(&[]);
            if providers.iter().all(|p| p.is_static()) {
                address.insert(id, (static_values.len() << 1) | 1);
                if same_providers(old_providers, providers) {
                    static_values.push(old.unwrap().facet_erased(facet.as_ref()).clone());
                } else {
                    let parts: Vec<(&AnyValue, bool)> = providers
                        .iter()
                        .map(|p| match &p.value {
                            ProviderValue::Static(v) => (v, false),
                            _ => unreachable!(),
                        })
                        .collect();
                    let value = facet.combine(&parts);
                    let value = match old {
                        Some(old) if facet.compare(&value, old.facet_erased(facet.as_ref())) => {
                            old.facet_erased(facet.as_ref()).clone()
                        }
                        _ => value,
                    };
                    static_values.push(value);
                }
            } else {
                for p in providers {
                    match &p.value {
                        ProviderValue::Static(v) => {
                            address.insert(p.id, (static_values.len() << 1) | 1);
                            static_values.push(v.clone());
                        }
                        _ => {
                            address.insert(p.id, makers.len() << 1);
                            makers.push(SlotMaker::Provider(p.clone()));
                        }
                    }
                }
                address.insert(id, makers.len() << 1);
                makers.push(SlotMaker::Facet(facet, providers.clone()));
            }
        }

        let dynamic_slots = makers
            .into_iter()
            .map(|maker| -> Box<dyn DynamicSlot> {
                match maker {
                    SlotMaker::Field(field) => Box::new(FieldSlot { field }),
                    SlotMaker::Provider(provider) => Box::new(ProviderSlot::new(provider, &address)),
                    SlotMaker::Facet(facet, providers) => Box::new(FacetSlot::new(facet, providers, &address)),
                }
            })
            .collect();

        Configuration { base, compartments: new_compartments, dynamic_slots, address, static_values, facets }
    }
}

fn same_providers(a: &[Arc<FacetProvider>], b: &[Arc<FacetProvider>]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.id == y.id)
}

fn flatten(
    extension: &Extension,
    compartments: &HashMap<u64, Extension>,
    new_compartments: &mut HashMap<u64, Extension>,
) -> Vec<Flat> {
    let mut result: [Vec<Flat>; 5] = Default::default();
    let mut seen: HashMap<(u8, usize), u8> = HashMap::new();

    fn inner(
        ext: &Extension,
        prec: u8,
        result: &mut [Vec<Flat>; 5],
        seen: &mut HashMap<(u8, usize), u8>,
        compartments: &HashMap<u64, Extension>,
        new_compartments: &mut HashMap<u64, Extension>,
    ) {
        let key = ext.key();
        if let Some(&known) = seen.get(&key) {
            if known <= prec {
                return;
            }
            if let Some(found) = result[known as usize].iter().position(|f| f.key() == key) {
                result[known as usize].remove(found);
            }
            if let Ext::Compartment(c) = &ext.0 {
                new_compartments.remove(&c.0.id);
            }
        }
        seen.insert(key, prec);
        match &ext.0 {
            Ext::List(list) => {
                for e in list.iter() {
                    inner(e, prec, result, seen, compartments, new_compartments);
                }
            }
            Ext::Compartment(c) => {
                let (compartment, default) = &**c;
                assert!(!new_compartments.contains_key(&compartment.id), "Duplicate use of compartment in extensions");
                let content = compartments.get(&compartment.id).unwrap_or(default).clone();
                new_compartments.insert(compartment.id, content.clone());
                inner(&content, prec, result, seen, compartments, new_compartments);
            }
            Ext::Prec(p) => inner(&p.0, p.1, result, seen, compartments, new_compartments),
            Ext::Field(f) => {
                result[prec as usize].push(Flat::Field(f.clone()));
                if let Some(provides) = f.provides() {
                    inner(&provides, prec, result, seen, compartments, new_compartments);
                }
            }
            Ext::Provider(p) => {
                result[prec as usize].push(Flat::Provider(p.clone()));
                if let Some(enabled) = p.facet.extensions() {
                    inner(&enabled, PREC_DEFAULT, result, seen, compartments, new_compartments);
                }
            }
        }
    }

    inner(extension, PREC_DEFAULT, &mut result, &mut seen, compartments, new_compartments);
    result.into_iter().flatten().collect()
}

fn ensure_all(state: &EditorState, addrs: &[usize]) -> bool {
    let mut changed = false;
    for &addr in addrs {
        if state.ensure_addr(addr) & STATUS_CHANGED != 0 {
            changed = true;
        }
    }
    changed
}

struct FieldSlot {
    field: Arc<dyn ErasedField>,
}

impl DynamicSlot for FieldSlot {
    fn create(&self, state: &EditorState) -> (AnyValue, bool) {
        (create_field(self.field.as_ref(), state), true)
    }

    fn update(&self, _state: &EditorState, tr: &Transaction, prev: &AnyValue) -> (AnyValue, bool) {
        let value = self.field.update(prev, tr);
        if self.field.compare(prev, &value) {
            (prev.clone(), false)
        } else {
            (value, true)
        }
    }

    fn reconfigure(&self, state: &EditorState, old: &EditorState) -> (AnyValue, bool) {
        let id = self.field.id();
        let init = state.facet(&INIT_FIELD).iter().find(|i| i.field == id);
        let old_init = old.facet(&INIT_FIELD).iter().find(|i| i.field == id);
        if let Some(init) = init {
            if old_init.map(|o| o.id) != Some(init.id) {
                return ((init.create)(state), true);
            }
        }
        if let Some(value) = old.field_erased(id) {
            return (value.clone(), false);
        }
        (create_field(self.field.as_ref(), state), true)
    }
}

struct ProviderSlot {
    provider: Arc<FacetProvider>,
    dep_doc: bool,
    dep_sel: bool,
    dep_addrs: Vec<usize>,
}

impl ProviderSlot {
    fn new(provider: Arc<FacetProvider>, address: &HashMap<u64, usize>) -> ProviderSlot {
        let (mut dep_doc, mut dep_sel, mut dep_addrs) = (false, false, Vec::new());
        for dep in &provider.deps {
            match &dep.0 {
                DepKind::Doc => dep_doc = true,
                DepKind::Selection => dep_sel = true,
                DepKind::Facet(f) => {
                    let addr = address.get(&f.id()).copied().unwrap_or(1);
                    if addr & 1 == 0 {
                        dep_addrs.push(addr);
                    }
                }
                DepKind::Field(id) => {
                    let addr = address.get(id).copied().unwrap_or(1);
                    if addr & 1 == 0 {
                        dep_addrs.push(addr);
                    }
                }
            }
        }
        ProviderSlot { provider, dep_doc, dep_sel, dep_addrs }
    }

    fn compare(&self, a: &AnyValue, b: &AnyValue) -> bool {
        self.provider.facet.compare_input(a, b, self.provider.is_multi())
    }
}

impl DynamicSlot for ProviderSlot {
    fn create(&self, state: &EditorState) -> (AnyValue, bool) {
        (self.provider.getter()(state), true)
    }

    fn update(&self, state: &EditorState, tr: &Transaction, prev: &AnyValue) -> (AnyValue, bool) {
        if (self.dep_doc && tr.doc_changed())
            || (self.dep_sel && (tr.doc_changed() || tr.selection().is_some()))
            || ensure_all(state, &self.dep_addrs)
        {
            let value = self.provider.getter()(state);
            if !self.compare(&value, prev) {
                return (value, true);
            }
        }
        (prev.clone(), false)
    }

    fn reconfigure(&self, state: &EditorState, old: &EditorState) -> (AnyValue, bool) {
        let Some(&old_addr) = old.config().address.get(&self.provider.id) else {
            return (self.provider.getter()(state), true);
        };
        let old_value = old.get_addr(old_addr).clone();
        let deps_same = self.provider.deps.iter().all(|dep| match &dep.0 {
            DepKind::Facet(f) => Arc::ptr_eq(old.facet_erased(f.as_ref()), state.facet_erased(f.as_ref())),
            DepKind::Field(id) => match (old.field_erased(*id), state.field_erased(*id)) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            },
            _ => true,
        });
        if deps_same {
            return (old_value, false);
        }
        let value = self.provider.getter()(state);
        if self.compare(&value, &old_value) {
            (old_value, false)
        } else {
            (value, true)
        }
    }
}

struct FacetSlot {
    facet: Arc<dyn ErasedFacet>,
    providers: Vec<Arc<FacetProvider>>,
    provider_addrs: Vec<usize>,
    dynamic: Vec<usize>,
}

impl FacetSlot {
    fn new(facet: Arc<dyn ErasedFacet>, providers: Vec<Arc<FacetProvider>>, address: &HashMap<u64, usize>) -> Self {
        let provider_addrs: Vec<usize> = providers.iter().map(|p| address[&p.id]).collect();
        let dynamic = provider_addrs.iter().copied().filter(|a| a & 1 == 0).collect();
        FacetSlot { facet, providers, provider_addrs, dynamic }
    }

    fn get(&self, state: &EditorState) -> AnyValue {
        let parts: Vec<(&AnyValue, bool)> = self
            .provider_addrs
            .iter()
            .zip(&self.providers)
            .map(|(&addr, p)| (state.get_addr(addr), p.is_multi()))
            .collect();
        self.facet.combine(&parts)
    }
}

impl DynamicSlot for FacetSlot {
    fn create(&self, state: &EditorState) -> (AnyValue, bool) {
        for &addr in &self.provider_addrs {
            state.ensure_addr(addr);
        }
        (self.get(state), true)
    }

    fn update(&self, state: &EditorState, _tr: &Transaction, prev: &AnyValue) -> (AnyValue, bool) {
        if !ensure_all(state, &self.dynamic) {
            return (prev.clone(), false);
        }
        let value = self.get(state);
        if self.facet.compare(&value, prev) {
            (prev.clone(), false)
        } else {
            (value, true)
        }
    }

    fn reconfigure(&self, state: &EditorState, old: &EditorState) -> (AnyValue, bool) {
        let dep_changed = ensure_all(state, &self.provider_addrs);
        let old_providers = old.config().facets.get(&self.facet.id());
        let old_value = old.facet_erased(self.facet.as_ref()).clone();
        if let Some(old_providers) = old_providers {
            if !dep_changed && same_providers(&self.providers, old_providers) {
                return (old_value, false);
            }
        }
        let value = self.get(state);
        if self.facet.compare(&value, &old_value) {
            (old_value, false)
        } else {
            (value, true)
        }
    }
}

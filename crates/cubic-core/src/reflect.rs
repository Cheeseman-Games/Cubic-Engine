//! Reflection for components: what a type *is*, and what can be edited.
//!
//! The editor knows nothing about the game's types — it must list an entity's
//! components, show their fields, and write a changed field back without ever
//! naming a concrete component type. This module is that contract, in three
//! pieces:
//!
//! - [`ComponentInfo`] gives a type its stable identity: the name a `.rsn`
//!   file and the inspector refer to it by, plus its [`TypeId`]. One
//!   `#[derive(Component)]` produces it.
//! - [`Inspectable`] describes a type's editable fields as static
//!   [`Field`] descriptors — name, [`FieldKind`], and type-erased `get`/`set`
//!   closures. One `#[derive(Inspectable)]` produces it.
//! - [`ComponentRegistry`] holds both for every component a project knows,
//!   so the inspector, the scene layer, and prefabs can go by name.
//!
//! Both derives come from [`cubic-derive`], and together they are how a game
//! makes its components visible to the editor:
//!
//! ```
//! use cubic_core::prelude::*;
//!
//! #[derive(Component, Inspectable)]
//! struct Health {
//!     current: f32,
//!     max: f32,
//!     alive: bool,
//! }
//!
//! let mut registry = ComponentRegistry::new();
//! registry.register::<Health>();
//! let health = registry.get("Health").expect("just registered");
//!
//! let mut world = World::new();
//! let entity = world.spawn();
//! world.insert(entity, Health { current: 80.0, max: 100.0, alive: true });
//!
//! // Every field is now readable and writable without naming `Health`:
//! let component = health.read(&world, entity).expect("it has one");
//! assert_eq!(health.fields[0].get(component), FieldValue::F32(80.0));
//! ```
//!
//! # Kinds
//!
//! [`FieldKind`] is the small vocabulary an inspector draws widgets from:
//! `f32`/`i32`/`bool` become drag values, [`Vec2`](crate::math::Vec2) a pair
//! of them, [`Color`](FieldKind::Color) a swatch, an asset handle a picker,
//! a [`Select`](FieldKind::Select) a combo box, a string a text box. The
//! kind carries its data — options for a select, what asset for an asset —
//! so a widget needs only the descriptor.
//!
//! # Where this sits
//!
//! Descriptors are static and type-erased on purpose: a descriptor costs one
//! `fn` pointer pair per field, is `Copy`, and can be handed across the
//! type boundary to the editor (a different crate) while still reading and
//! writing the field it describes. Nothing here allocates per read, and the
//! [`scene`](crate::scene) layer can sit on the same descriptors later
//! instead of keeping a parallel description of every component.

use std::any::{Any, TypeId};
use std::collections::BTreeMap;
use std::fmt;

pub use cubic_derive::{Component, Inspectable};

use crate::assets::AssetHandle;
use crate::components::Transform;
use crate::hierarchy::Parent;
use crate::math::Vec2;
use crate::render::Rgba;
use crate::world::{EntityId, World};

/// A component type's stable identity.
///
/// `name` is the file- and UI-facing handle: what a `.rsn` scene writes, what
/// the inspector's header shows, and what a registry keys on. It therefore
/// must not change once a project has scenes — rename the Rust type freely,
/// but keep a name that files already carry.
///
/// Produced by `#[derive(Component)]`.
pub trait ComponentInfo {
    /// The name this type is known by outside the type system.
    fn name() -> &'static str;

    /// This type's [`TypeId`], for registries that key structurally.
    fn type_id() -> TypeId;
}

/// A component type's editable shape.
///
/// `fields` are in declaration order, one per field the derive did not skip,
/// each carrying enough to read it, write it, and draw a widget for it.
///
/// Produced by `#[derive(Inspectable)]`.
pub trait Inspectable {
    /// This type's fields, statically allocated and shared by every caller.
    fn fields() -> &'static [Field];
}

/// The variant index of a field annotated `#[inspect(select = [...])]`.
///
/// The macro cannot convert an arbitrary Rust enum to and from an index, so a
/// field like that declares this itself: `index` names the variant's position
/// in the option list the annotation spells out, and `from_index` is the
/// inverse, called only for an index the annotation knew about.
///
/// ```ignore
/// enum PlayerState { Idle, Walk, Run }
///
/// impl Selectable for PlayerState {
///     fn index(self) -> usize {
///         match self {
///             Self::Idle => 0,
///             Self::Walk => 1,
///             Self::Run => 2,
///         }
///     }
///
///     fn from_index(index: usize) -> Self {
///         [Self::Idle, Self::Walk, Self::Run][index]
///     }
/// }
/// ```
pub trait Selectable: Copy {
    /// This variant's position in the option list its field's annotation
    /// declares.
    fn index(self) -> usize;

    /// The variant at `index` in that list.
    ///
    /// The descriptor refuses an index outside the list before calling this,
    /// so an implementation may index its variants directly.
    fn from_index(index: usize) -> Self;
}

/// What one editable field of a component is.
///
/// A descriptor pairs a name and a [`FieldKind`] with the closures that read
/// and write the field through `Any` — the whole reason a field can be
/// edited without the editor knowing the component's type. Constructed by
/// `#[derive(Inspectable)]`, or by hand for a type that cannot derive it.
#[derive(Clone, Copy)]
pub struct Field {
    name: &'static str,
    kind: FieldKind,
    get: fn(&dyn Any) -> FieldValue,
    set: fn(&mut dyn Any, &FieldValue) -> Result<(), FieldError>,
}

impl Field {
    /// A field descriptor.
    ///
    /// `get` and `set` downcast `any` to the component type and read or write
    /// the field; they are only ever called with the component type the
    /// registry routed, so a downcast miss is a bug in the routing, not a
    /// case to handle.
    pub const fn new(
        name: &'static str,
        kind: FieldKind,
        get: fn(&dyn Any) -> FieldValue,
        set: fn(&mut dyn Any, &FieldValue) -> Result<(), FieldError>,
    ) -> Self {
        Self {
            name,
            kind,
            get,
            set,
        }
    }

    /// The field's name, as the inspector shows it.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// What kind of value this field holds, and the widget it implies.
    pub fn kind(&self) -> FieldKind {
        self.kind
    }

    /// Reads the field out of a type-erased component.
    pub fn get(&self, component: &dyn Any) -> FieldValue {
        (self.get)(component)
    }

    /// Writes `value` into the field of a type-erased component.
    ///
    /// Fails only if `value` is a different kind than this field — a widget
    /// handing back the wrong variant is the case this catches.
    pub fn set(&self, component: &mut dyn Any, value: &FieldValue) -> Result<(), FieldError> {
        (self.set)(component, value)
    }
}

impl fmt::Debug for Field {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Field")
            .field("name", &self.name)
            .field("kind", &self.kind)
            .finish()
    }
}

/// The kinds of value a [`Field`] can hold, and therefore the widgets an
/// inspector draws for them.
///
/// Each variant carries the data its widget needs, so an inspector needs no
/// second lookup: the options of a [`Select`](Self::Select) and the asset
/// family of an [`Asset`](Self::Asset) travel with the descriptor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    /// A float: a drag-scrub value.
    F32,
    /// An integer: a drag-scrub value with a step of 1.
    I32,
    /// A checkbox.
    Bool,
    /// A 2D vector: two drag-scrub values.
    Vec2,
    /// An [`Rgba`] color: a color swatch.
    Color,
    /// A handle to an asset of `kind`: a picker over that asset family.
    Asset {
        /// Which asset family the picker offers.
        kind: crate::assets::AssetKind,
    },
    /// One of `labels`, by index: a combo box.
    Select(&'static [&'static str]),
    /// Free text: a text box.
    String,
}

/// A field's current value, type-erased at the boundary.
///
/// This is what crosses between the editor and a component: `get` hands one
/// out, `set` takes one back. It is the union of what [`FieldKind`] describes,
/// so a widget and its descriptor always agree on the shape.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldValue {
    /// An [`FieldKind::F32`] value.
    F32(f32),
    /// An [`FieldKind::I32`] value.
    I32(i32),
    /// A [`FieldKind::Bool`] value.
    Bool(bool),
    /// A [`FieldKind::Vec2`] value.
    Vec2(Vec2),
    /// A [`FieldKind::Color`] value.
    Color(Rgba),
    /// A [`FieldKind::Asset`] value — the handle, not its path.
    Asset(AssetHandle),
    /// A [`FieldKind::Select`] value — the option's index.
    Select(usize),
    /// A [`FieldKind::String`] value.
    String(String),
}

impl FieldValue {
    fn wrong_kind(&self, expected: &'static str) -> FieldError {
        FieldError::WrongKind {
            expected,
            found: self.name(),
        }
    }

    /// This value's own kind name, for a mismatch error.
    pub fn name(&self) -> &'static str {
        match self {
            Self::F32(_) => "f32",
            Self::I32(_) => "i32",
            Self::Bool(_) => "bool",
            Self::Vec2(_) => "Vec2",
            Self::Color(_) => "color",
            Self::Asset(_) => "asset",
            Self::Select(_) => "select",
            Self::String(_) => "string",
        }
    }

    /// The float, or the [`WrongKind`](FieldError::WrongKind) it is not.
    pub fn f32(&self) -> Result<f32, FieldError> {
        match self {
            Self::F32(value) => Ok(*value),
            other => Err(other.wrong_kind("f32")),
        }
    }

    /// The integer, or the [`WrongKind`](FieldError::WrongKind) it is not.
    pub fn i32(&self) -> Result<i32, FieldError> {
        match self {
            Self::I32(value) => Ok(*value),
            other => Err(other.wrong_kind("i32")),
        }
    }

    /// The flag, or the [`WrongKind`](FieldError::WrongKind) it is not.
    pub fn boolean(&self) -> Result<bool, FieldError> {
        match self {
            Self::Bool(value) => Ok(*value),
            other => Err(other.wrong_kind("bool")),
        }
    }

    /// The vector, or the [`WrongKind`](FieldError::WrongKind) it is not.
    pub fn vec2(&self) -> Result<Vec2, FieldError> {
        match self {
            Self::Vec2(value) => Ok(*value),
            other => Err(other.wrong_kind("Vec2")),
        }
    }

    /// The color, or the [`WrongKind`](FieldError::WrongKind) it is not.
    pub fn color(&self) -> Result<Rgba, FieldError> {
        match self {
            Self::Color(value) => Ok(*value),
            other => Err(other.wrong_kind("color")),
        }
    }

    /// The asset handle, or the [`WrongKind`](FieldError::WrongKind) it is not.
    pub fn asset(&self) -> Result<AssetHandle, FieldError> {
        match self {
            Self::Asset(value) => Ok(*value),
            other => Err(other.wrong_kind("asset")),
        }
    }

    /// The selected option's index, or the [`WrongKind`](FieldError::WrongKind)
    /// it is not.
    pub fn select(&self) -> Result<usize, FieldError> {
        match self {
            Self::Select(value) => Ok(*value),
            other => Err(other.wrong_kind("select")),
        }
    }

    /// The text, or the [`WrongKind`](FieldError::WrongKind) it is not.
    pub fn string(&self) -> Result<String, FieldError> {
        match self {
            Self::String(value) => Ok(value.clone()),
            other => Err(other.wrong_kind("string")),
        }
    }
}

impl fmt::Display for FieldValue {
    /// The value as a single line of text — the inspector's read-only form,
    /// and what a test asserts against.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::F32(value) => write!(f, "{value}"),
            Self::I32(value) => write!(f, "{value}"),
            Self::Bool(value) => write!(f, "{value}"),
            Self::Vec2(value) => write!(f, "({}, {})", value.x, value.y),
            Self::Color(value) => write!(f, "({}, {}, {}, {})", value.r, value.g, value.b, value.a),
            Self::Asset(value) => write!(f, "{value}"),
            Self::Select(value) => write!(f, "#{value}"),
            Self::String(value) => write!(f, "{value}"),
        }
    }
}

/// Why a field could not be read or written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldError {
    /// The value handed to `set` was not the kind this field holds.
    WrongKind {
        /// The kind the field wants.
        expected: &'static str,
        /// The kind the value actually is.
        found: &'static str,
    },
    /// A `select` field was set to an index outside its option list.
    SelectOutOfRange(usize),
}

impl fmt::Display for FieldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongKind { expected, found } => {
                write!(f, "this field holds {expected}, not {found}")
            }
            Self::SelectOutOfRange(index) => {
                write!(f, "option #{index} is outside this field's option list")
            }
        }
    }
}

impl std::error::Error for FieldError {}

/// How a component type looks to code that cannot name it.
///
/// The registry stores one of these per registered component; the closures
/// are the only place the concrete type still exists.
type ReadFn = for<'a> fn(&'a World, EntityId) -> Option<&'a dyn Any>;
type ReadMutFn = for<'a> fn(&'a mut World, EntityId) -> Option<&'a mut dyn Any>;
/// Takes a component off an entity, reporting whether there was one.
type RemoveFn = fn(&mut World, EntityId) -> bool;
/// Gives an entity the type's default instance, for an "add component" menu.
type DefaultInsertFn = fn(&mut World, EntityId);

/// One registered component type: its identity, its fields, and how to reach
/// it inside a [`World`].
///
/// Handed out by [`ComponentRegistry::get`] and
/// [`ComponentRegistry::on_entity`], and used by the inspector to show an
/// entity's components by name — the concrete type never leaves the registry.
#[derive(Clone, Copy)]
pub struct ComponentEntry {
    /// The type's stable name (its [`ComponentInfo::name`]).
    pub name: &'static str,
    /// The type's [`TypeId`].
    pub type_id: TypeId,
    /// Its editable fields, in declaration order.
    pub fields: &'static [Field],
    read: ReadFn,
    read_mut: ReadMutFn,
    remove: RemoveFn,
    /// `Some` when the type can be given to an entity from scratch —
    /// [`register_addable`](ComponentRegistry::register_addable). Structural
    /// components the editor must not offer to add (`Parent`, say) leave this
    /// `None`.
    default_insert: Option<DefaultInsertFn>,
}

impl fmt::Debug for ComponentEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ComponentEntry")
            .field("name", &self.name)
            .field("type_id", &self.type_id)
            .field("fields", &self.fields)
            .finish()
    }
}

impl ComponentEntry {
    /// The field called `name`, if this type has one.
    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|field| field.name == name)
    }

    /// The type-erased component `entity` holds, if it holds this one.
    pub fn read<'a>(&self, world: &'a World, entity: EntityId) -> Option<&'a dyn Any> {
        (self.read)(world, entity)
    }

    /// The type-erased component `entity` holds, for writing.
    ///
    /// Mutable access, so an inspector can apply a field edit in place.
    pub fn read_mut<'a>(&self, world: &'a mut World, entity: EntityId) -> Option<&'a mut dyn Any> {
        (self.read_mut)(world, entity)
    }

    /// Whether `entity` holds this component.
    pub fn has(&self, world: &World, entity: EntityId) -> bool {
        self.read(world, entity).is_some()
    }

    /// Writes `value` into `field` of `entity`'s component of this type.
    ///
    /// `Ok(None)` means the entity does not hold the component at all — an
    /// edit to something that is gone, which the caller reports rather than
    /// inventing a component to write into. `Ok(Some(()))` is a change
    /// applied.
    pub fn set_field(
        &self,
        world: &mut World,
        entity: EntityId,
        field: &Field,
        value: &FieldValue,
    ) -> Result<Option<()>, FieldError> {
        let Some(component) = (self.read_mut)(world, entity) else {
            return Ok(None);
        };
        field.set(component, value).map(Some)
    }

    /// Takes this component off `entity`, reporting whether it held one.
    ///
    /// This is the generic half of an editor's "remove component" menu: the
    /// same call removes a `Transform` or a `Parent`, and a caller that must
    /// be able to undo the removal should capture the component first (see
    /// [`SceneRegistry::snapshot_entity`](crate::scene::SceneRegistry::snapshot_entity)).
    pub fn remove(&self, world: &mut World, entity: EntityId) -> bool {
        (self.remove)(world, entity)
    }

    /// Whether [`add_default`](Self::add_default) has something to add.
    ///
    /// False for components an entity cannot pick up out of a menu — ones
    /// only a command that knows what it is doing should write, because a
    /// bare default of them would be meaningless or wrong.
    pub fn is_addable(&self) -> bool {
        self.default_insert.is_some()
    }

    /// Gives `entity` a fresh default of this component, reporting success.
    ///
    /// Fails (`false`) for a component that is not addable — see
    /// [`is_addable`](Self::is_addable). Succeeds even if the entity already
    /// holds one: the default replaces it, which is what "reset to a new one"
    /// should do.
    pub fn add_default(&self, world: &mut World, entity: EntityId) -> bool {
        match self.default_insert {
            Some(add) => {
                add(world, entity);
                true
            }
            None => false,
        }
    }
}

/// Every component type a project's editor knows, by name.
///
/// The registry is how "the inspector lists this entity's components" and
/// "a `.rsn` file knows what a `Transform` is" become the same fact: one
/// place a game registers its types once, and one lookup for everything that
/// must address them without naming them.
///
/// Keys are sorted ([`BTreeMap`]), so listings and saved forms are stable.
#[derive(Default)]
pub struct ComponentRegistry {
    entries: BTreeMap<&'static str, ComponentEntry>,
}

impl fmt::Debug for ComponentRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ComponentRegistry")
            .field("names", &self.names().collect::<Vec<_>>())
            .finish()
    }
}

impl ComponentRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `T`, keyed by its [`ComponentInfo::name`].
    ///
    /// Deriving both `Component` and `Inspectable` is what makes this legal —
    /// the entry needs the type's identity *and* its field descriptors.
    /// Registering the same name again replaces the previous entry, so a
    /// re-registration (a hot-reloaded system) is idempotent.
    ///
    /// What this registers is not addable: the type is read, edited and
    /// removed by name, but an "add component" menu will not offer it. Use
    /// [`register_addable`](Self::register_addable) for a type an entity can
    /// pick up as a fresh default.
    pub fn register<T>(&mut self) -> &mut Self
    where
        T: Any + ComponentInfo + Inspectable,
    {
        self.insert_entry::<T>(None)
    }

    /// Registers `T` the way [`register`](Self::register) does, plus a
    /// default instance for [`add_default`](ComponentEntry::add_default).
    ///
    /// The `Default` bound is the whole difference: a menu can hand `T` to
    /// an entity it has never held, and a command that adds one knows its
    /// rollback is a plain remove.
    pub fn register_addable<T>(&mut self) -> &mut Self
    where
        T: Any + ComponentInfo + Inspectable + Default,
    {
        self.insert_entry::<T>(Some(add_default::<T>))
    }

    fn insert_entry<T>(&mut self, default_insert: Option<DefaultInsertFn>) -> &mut Self
    where
        T: Any + ComponentInfo + Inspectable,
    {
        self.entries.insert(
            T::name(),
            ComponentEntry {
                name: T::name(),
                type_id: <T as ComponentInfo>::type_id(),
                fields: T::fields(),
                read: read_component::<T>,
                read_mut: read_component_mut::<T>,
                remove: remove_component::<T>,
                default_insert,
            },
        );
        self
    }

    /// The entry registered under `name`.
    pub fn get(&self, name: &str) -> Option<&ComponentEntry> {
        self.entries.get(name)
    }

    /// Every registered name, sorted.
    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.entries.keys().copied()
    }

    /// Every registered entry, in name order.
    pub fn entries(&self) -> impl Iterator<Item = &ComponentEntry> {
        self.entries.values()
    }

    /// How many component types are registered.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no component types are registered.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The components `entity` actually holds, in name order.
    ///
    /// This is the inspector's path from a selection to what to draw: a
    /// registered type the entity does not have is simply absent, not an
    /// error, so the listing is whatever the entity carries.
    pub fn on_entity(&self, world: &World, entity: EntityId) -> Vec<&ComponentEntry> {
        self.entries
            .values()
            .filter(|entry| entry.has(world, entity))
            .collect()
    }

    /// The engine's own components, ready to edit with.
    ///
    /// A game adds its types on top of this before opening a scene of theirs.
    /// `Transform` is addable; `Parent` is not — hierarchy is built by
    /// reparenting, never by handing an entity a bare parent id.
    pub fn engine_defaults() -> Self {
        let mut registry = Self::new();
        registry.register_addable::<Transform>();
        registry.register::<Parent>();
        registry
    }
}

/// The `read` closure registered for `T`.
fn read_component<T: Any>(world: &World, entity: EntityId) -> Option<&dyn Any> {
    world
        .get::<T>(entity)
        .map(|component| component as &dyn Any)
}

/// The `read_mut` closure registered for `T`.
fn read_component_mut<T: Any>(world: &mut World, entity: EntityId) -> Option<&mut dyn Any> {
    world
        .get_mut::<T>(entity)
        .map(|component| component as &mut dyn Any)
}

/// The `remove` closure registered for `T`.
fn remove_component<T: Any>(world: &mut World, entity: EntityId) -> bool {
    world.remove::<T>(entity).is_some()
}

/// The `default_insert` closure registered for an addable `T`.
fn add_default<T: Default + 'static>(world: &mut World, entity: EntityId) {
    world.insert(entity, T::default());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Vec2;

    /// A component registered through the public API, without going through
    /// the derives (those are exercised end to end in `tests/reflect.rs`).
    struct Health {
        current: f32,
        alive: bool,
        #[allow(dead_code)]
        label: String,
    }

    fn current_get(component: &dyn Any) -> FieldValue {
        let health = component.downcast_ref::<Health>().expect("Health");
        FieldValue::F32(health.current)
    }

    fn current_set(component: &mut dyn Any, value: &FieldValue) -> Result<(), FieldError> {
        let health = component.downcast_mut::<Health>().expect("Health");
        health.current = value.f32()?;
        Ok(())
    }

    fn alive_get(component: &dyn Any) -> FieldValue {
        let health = component.downcast_ref::<Health>().expect("Health");
        FieldValue::Bool(health.alive)
    }

    fn alive_set(component: &mut dyn Any, value: &FieldValue) -> Result<(), FieldError> {
        let health = component.downcast_mut::<Health>().expect("Health");
        health.alive = value.boolean()?;
        Ok(())
    }

    static HEALTH_FIELDS: &[Field] = &[
        Field::new("current", FieldKind::F32, current_get, current_set),
        Field::new("alive", FieldKind::Bool, alive_get, alive_set),
    ];

    impl ComponentInfo for Health {
        fn name() -> &'static str {
            "Health"
        }

        fn type_id() -> TypeId {
            TypeId::of::<Health>()
        }
    }

    impl Inspectable for Health {
        fn fields() -> &'static [Field] {
            HEALTH_FIELDS
        }
    }

    fn registry() -> ComponentRegistry {
        let mut registry = ComponentRegistry::new();
        registry.register::<Health>();
        registry
    }

    fn world_with_health() -> World {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(
            entity,
            Health {
                current: 80.0,
                alive: true,
                label: "hero".to_owned(),
            },
        );
        world
    }

    #[test]
    fn a_registered_component_is_addressable_by_its_derived_name() {
        let registry = registry();
        let entry = registry.get("Health").expect("registered");
        assert_eq!(entry.name, "Health");
        assert_eq!(entry.type_id, TypeId::of::<Health>());
        assert_eq!(registry.names().collect::<Vec<_>>(), ["Health"]);
        assert_eq!(registry.len(), 1);
        assert!(!registry.is_empty());
    }

    #[test]
    fn fields_carry_a_name_and_the_kind_a_widget_draws() {
        let registry = registry();
        let entry = registry.get("Health").unwrap();
        let names: Vec<&str> = entry.fields.iter().map(Field::name).collect();
        let kinds: Vec<FieldKind> = entry.fields.iter().map(Field::kind).collect();
        assert_eq!(names, ["current", "alive"]);
        assert_eq!(kinds, [FieldKind::F32, FieldKind::Bool]);
        assert_eq!(
            entry.field("current").map(Field::name),
            Some("current"),
            "fields are also findable by name"
        );
        assert!(entry.field("nope").is_none());
    }

    /// The whole point: read and write a field without naming the type.
    #[test]
    fn a_field_can_be_read_and_written_through_the_type_erased_entry() {
        let registry = registry();
        let entry = registry.get("Health").unwrap();
        let mut world = world_with_health();
        let entity = world.entities()[0];

        let component = entry.read(&world, entity).expect("it holds one");
        assert_eq!(entry.fields[0].get(component), FieldValue::F32(80.0));
        assert_eq!(
            entry.fields[1].get(component),
            FieldValue::Bool(true),
            "each descriptor reads its own field"
        );

        let component = entry.read_mut(&mut world, entity).unwrap();
        entry.fields[0]
            .set(component, &FieldValue::F32(42.0))
            .expect("f32 into an f32 field");
        assert_eq!(
            world.get::<Health>(entity).map(|health| health.current),
            Some(42.0),
            "the edit landed in the world"
        );
    }

    #[test]
    fn set_field_reports_a_missing_component_and_a_wrong_kind() {
        let registry = registry();
        let entry = registry.get("Health").unwrap();
        let mut world = World::new();
        let held = world.spawn();
        let bare = world.spawn();
        world.insert(
            held,
            Health {
                current: 1.0,
                alive: false,
                label: String::new(),
            },
        );

        assert_eq!(
            entry.set_field(&mut world, bare, &entry.fields[0], &FieldValue::F32(9.0)),
            Ok(None),
            "an entity without the component is Ok(None), not an error"
        );
        assert_eq!(
            entry.set_field(&mut world, held, &entry.fields[0], &FieldValue::Bool(false)),
            Err(FieldError::WrongKind {
                expected: "f32",
                found: "bool"
            }),
            "the wrong kind of value is refused before it touches the component"
        );
        assert_eq!(
            entry.set_field(&mut world, held, &entry.fields[0], &FieldValue::F32(9.0)),
            Ok(Some(()))
        );
        assert_eq!(world.get::<Health>(held).unwrap().current, 9.0);
    }

    #[test]
    fn on_entity_lists_only_the_components_the_entity_holds() {
        let mut registry = ComponentRegistry::new();
        registry.register::<Health>();
        registry.register::<Transform>();
        let mut world = world_with_health();
        let with_transform = world.spawn();
        world.insert(
            with_transform,
            Health {
                current: 10.0,
                alive: false,
                label: String::new(),
            },
        );
        world.insert(with_transform, Transform::default());
        let health_only = world.entities()[0];

        let names = |world: &World, entity: EntityId| {
            registry
                .on_entity(world, entity)
                .into_iter()
                .map(|entry| entry.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&world, with_transform), ["Health", "Transform"]);
        assert_eq!(names(&world, health_only), ["Health"]);
        assert!(
            ComponentRegistry::new()
                .on_entity(&World::new(), with_transform)
                .is_empty(),
            "an unregistered component type never appears"
        );
    }

    #[test]
    fn registry_names_are_sorted_so_listings_are_stable() {
        let mut registry = ComponentRegistry::new();
        registry.register::<Transform>();
        registry.register::<Health>();
        assert_eq!(
            registry.names().collect::<Vec<_>>(),
            ["Health", "Transform"]
        );
        assert_eq!(
            registry.entries().map(|e| e.name).collect::<Vec<_>>(),
            ["Health", "Transform"]
        );
    }

    #[test]
    fn re_registering_a_name_replaces_the_entry() {
        let mut registry = registry();
        registry.register::<Health>();
        assert_eq!(registry.len(), 1, "one name, one entry");
    }

    #[test]
    fn an_empty_registry_says_so() {
        let registry = ComponentRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
        assert!(registry.get("Health").is_none());
    }

    #[test]
    fn engine_defaults_registers_transform() {
        let registry = ComponentRegistry::engine_defaults();
        let entry = registry
            .get("Transform")
            .expect("Transform is engine-default");
        assert_eq!(entry.type_id, TypeId::of::<Transform>());
        assert_eq!(
            entry.fields.iter().map(Field::name).collect::<Vec<_>>(),
            ["position", "rotation", "scale"],
            "Transform's fields come from its derive"
        );
        assert_eq!(
            entry.fields.iter().map(Field::kind).collect::<Vec<_>>(),
            [FieldKind::Vec2, FieldKind::F32, FieldKind::Vec2]
        );
    }

    #[test]
    fn field_values_reject_reads_of_the_wrong_kind() {
        assert_eq!(FieldValue::F32(1.0).f32(), Ok(1.0));
        assert_eq!(
            FieldValue::F32(1.0).boolean(),
            Err(FieldError::WrongKind {
                expected: "bool",
                found: "f32"
            })
        );
        assert_eq!(FieldValue::Bool(true).boolean(), Ok(true));
        assert_eq!(FieldValue::I32(3).i32(), Ok(3));
        assert_eq!(
            FieldValue::Vec2(Vec2::ONE).vec2(),
            Ok(Vec2::ONE),
            "the vector comes back whole"
        );
        assert_eq!(
            FieldValue::Color(Rgba::rgb(1.0, 0.5, 0.0))
                .color()
                .unwrap()
                .g,
            0.5
        );
        assert_eq!(FieldValue::Select(2).select(), Ok(2));
        assert_eq!(
            FieldValue::String("hi".to_owned()).string(),
            Ok("hi".to_owned()),
            "text is cloned out, so the widget can take ownership"
        );
        assert_eq!(
            FieldValue::Asset(AssetHandle::new(7u64.try_into().unwrap())).asset(),
            Ok(AssetHandle::new(7u64.try_into().unwrap()))
        );
    }

    #[test]
    fn field_values_render_as_one_line_for_the_inspector() {
        assert_eq!(FieldValue::F32(1.5).to_string(), "1.5");
        assert_eq!(FieldValue::I32(-3).to_string(), "-3");
        assert_eq!(FieldValue::Bool(false).to_string(), "false");
        assert_eq!(FieldValue::Vec2(Vec2::new(3.0, 4.0)).to_string(), "(3, 4)");
        assert_eq!(FieldValue::Select(1).to_string(), "#1");
        assert_eq!(FieldValue::String("hi".to_owned()).to_string(), "hi");
        assert_eq!(
            FieldValue::Asset(AssetHandle::new(7u64.try_into().unwrap())).to_string(),
            "asset#7"
        );
        assert_eq!(
            FieldValue::Color(Rgba::rgb(0.25, 0.5, 1.0)).to_string(),
            "(0.25, 0.5, 1, 1)"
        );
    }

    #[test]
    fn remove_takes_a_component_off_an_entity_by_name_alone() {
        let registry = registry();
        let entry = registry.get("Health").unwrap();
        let mut world = world_with_health();
        let entity = world.entities()[0];

        assert!(entry.remove(&mut world, entity), "it was there");
        assert!(!entry.has(&world, entity));
        assert!(
            !entry.remove(&mut world, entity),
            "the second removal finds nothing, but is not an error"
        );
        assert!(
            !world.entities().contains(&entity),
            "with its last component gone there is nothing left to list, the same \
             rule a scene save applies"
        );
    }

    #[test]
    fn only_addable_entries_hand_out_a_default() {
        let registry = ComponentRegistry::engine_defaults();
        let transform = registry.get("Transform").unwrap();
        let parent = registry.get("Parent").unwrap();

        assert!(transform.is_addable());
        assert!(!parent.is_addable(), "hierarchy is built by reparenting");

        let mut world = World::new();
        let entity = world.spawn();
        assert!(transform.add_default(&mut world, entity));
        assert_eq!(world.get::<Transform>(entity), Some(&Transform::default()));
        assert!(
            !parent.add_default(&mut world, entity),
            "an addable refusal changes nothing"
        );
        assert_eq!(world.get::<Parent>(entity), None);
    }

    #[test]
    fn a_non_addable_type_can_still_be_read_and_removed() {
        let registry = ComponentRegistry::engine_defaults();
        let parent = registry.get("Parent").unwrap();
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, crate::hierarchy::Parent { entity: 9 });

        assert!(parent.has(&world, entity));
        assert!(parent.read(&world, entity).is_some(), "it holds one");
        assert_eq!(parent.fields.len(), 0, "the link itself is not a field");
        assert!(parent.remove(&mut world, entity));
        assert!(!parent.has(&world, entity));
    }

    #[test]
    fn field_errors_explain_themselves() {
        assert_eq!(
            FieldError::WrongKind {
                expected: "f32",
                found: "string"
            }
            .to_string(),
            "this field holds f32, not string"
        );
        assert_eq!(
            FieldError::SelectOutOfRange(5).to_string(),
            "option #5 is outside this field's option list"
        );
    }

    #[test]
    fn field_kind_and_field_debug_for_diagnostics() {
        let field = Field::new("current", FieldKind::F32, current_get, current_set);
        assert_eq!(
            format!("{field:?}"),
            r#"Field { name: "current", kind: F32 }"#
        );
        assert_eq!(
            format!(
                "{:?}",
                FieldKind::Asset {
                    kind: crate::assets::AssetKind::Texture
                }
            ),
            r#"Asset { kind: Texture }"#
        );
        assert_eq!(
            FieldKind::Select(&["a", "b"]),
            FieldKind::Select(&["a", "b"]),
            "select kinds compare by their labels"
        );
        assert_eq!(
            FieldValue::name(&FieldValue::Select(0)),
            "select",
            "the mismatch error can name what it was handed"
        );
    }
}

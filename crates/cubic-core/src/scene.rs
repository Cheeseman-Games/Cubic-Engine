//! Scenes: the serialized form of a [`World`], and the `.rsn` save contract.
//!
//! A scene is a versioned snapshot of entities and their components. It exists
//! so a level can ship, and its text can grow, without dragging engine
//! internals — `TypeId`s, store layout, free lists — into the file format.
//!
//! # Text format
//!
//! Scenes are RON tuned for git diffs: one component per line, one entity per
//! block, every registered value kept on a single line so a one-field tweak is
//! exactly one changed line. A scene with two entities reads like this:
//!
//! ```ron
//! Scene(
//!     version: 1,
//!     entities: [
//!         SceneEntity(
//!             id: 0,
//!             components: [
//! SceneComponent(
//!                     name: "Transform",
//!                     value: (position: (3.0, 4.0), rotation: 0.0, scale: (1.0, 1.0)),
//!                 ),
//!             ],
//!         ),
//!         SceneEntity(id: 1, components: []),
//!     ],
//! )
//! ```
//!
//! Writing is deterministic — LF newlines on every OS, registry name order,
//! ascending entity ids — so identical worlds save to identical bytes, which
//! is what makes `.rsn` files diffable and mergeable by hand.

use std::any::Any;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;
use std::str::FromStr;

use ron::Error as RonError;
use ron::options::Options;
use ron::ser::PrettyConfig;
use ron::value::RawValue;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::components::Transform;
use crate::hierarchy::Parent;
use crate::world::{EntityId, World};

/// How a component type travels between a `World` and a scene file.
///
/// A codec knows the Rust type `T` on both sides of the boundary:
/// `encode` pulls `T` out of a world (if present) and turns it into a
/// single-line [`RawValue`]; `insert` pushes a `RawValue` back in.
/// Registration is erased here — the registry's `BTreeMap` needs a common
/// shape, and this is it.
type Encode = fn(&World, EntityId) -> Option<Result<Box<RawValue>, RonError>>;
type Insert = fn(&mut World, EntityId, &RawValue) -> Result<(), RonError>;

struct Codec {
    encode: Encode,
    insert: Insert,
}

fn encode_component<T: Any + Serialize>(
    world: &World,
    id: EntityId,
) -> Option<Result<Box<RawValue>, RonError>> {
    world.get::<T>(id).map(RawValue::from_rust)
}

fn insert_component<T: Any + DeserializeOwned>(
    world: &mut World,
    id: EntityId,
    value: &RawValue,
) -> Result<(), RonError> {
    let component: T = value.into_rust().map_err(RonError::from)?;
    world.insert(id, component);
    Ok(())
}

/// Name-to-codec map, so a file's component names resolve to Rust types.
///
/// The registry is deliberately a runtime object rather than a macro or
/// type-level list: a game can register its own components next to the
/// engine's, and a newer build can keep reading a scene by registering a
/// component whose contents it skipped as unknown before.
#[derive(Default)]
pub struct SceneRegistry {
    codecs: BTreeMap<String, Codec>,
}

impl SceneRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers component type `T` under the stable file name `name`.
    ///
    /// `name` is what appears in a scene file and therefore what must never
    /// change once a level ships; it can differ from the Rust type name if a
    /// rename keeps the file stable. Registering the same `name` again
    /// replaces the old codec.
    pub fn register<T>(&mut self, name: &str) -> &mut Self
    where
        T: Any + Serialize + DeserializeOwned,
    {
        self.codecs.insert(
            name.to_owned(),
            Codec {
                encode: encode_component::<T>,
                insert: insert_component::<T>,
            },
        );
        self
    }

    /// The registry of the engine's own components, ready for a game that
    /// loads or saves scenes of engine types. Games add their own types before
    /// touching a scene of theirs.
    pub fn engine_defaults() -> Self {
        let mut registry = Self::new();
        registry.register::<Transform>("Transform");
        registry.register::<Parent>("Parent");
        registry
    }

    /// Every registered component `entity` holds, in name order.
    ///
    /// This is the editor's capture step: a command that is about to change an
    /// entity takes one of these first, so its rollback can put the entity
    /// back exactly as it was — including the components nothing in the
    /// command touched.
    pub fn snapshot_entity(
        &self,
        world: &World,
        entity: EntityId,
    ) -> Result<Vec<SceneComponent>, SceneError> {
        let mut components = Vec::new();
        for (name, codec) in &self.codecs {
            match (codec.encode)(world, entity) {
                None => {}
                Some(Ok(value)) => components.push(SceneComponent {
                    name: name.clone(),
                    value,
                }),
                Some(Err(error)) => {
                    return Err(SceneError::Component {
                        entity,
                        name: name.clone(),
                        message: error.to_string(),
                    });
                }
            }
        }
        Ok(components)
    }

    /// Whether `name` has a codec, i.e. whether a save would carry it.
    pub fn has(&self, name: &str) -> bool {
        self.codecs.contains_key(name)
    }

    /// One registered component of `entity`, captured as its saved text.
    ///
    /// `Ok(None)` means the registry has no codec under `name`, or the entity
    /// does not hold that component; `Err` means the codec ran and the
    /// component could not be encoded.
    pub fn encode(
        &self,
        world: &World,
        entity: EntityId,
        name: &str,
    ) -> Result<Option<SceneComponent>, SceneError> {
        let Some(codec) = self.codecs.get(name) else {
            return Ok(None);
        };
        match (codec.encode)(world, entity) {
            None => Ok(None),
            Some(Ok(value)) => Ok(Some(SceneComponent {
                name: name.to_owned(),
                value,
            })),
            Some(Err(error)) => Err(SceneError::Component {
                entity,
                name: name.to_owned(),
                message: error.to_string(),
            }),
        }
    }

    /// Pushes a captured component back into `entity`.
    ///
    /// The inverse of [`snapshot`](Self::snapshot_entity): an unknown `name`
    /// is refused rather than silently ignored, because a rollback that drops
    /// a component it promised to restore would corrupt the scene quietly.
    pub fn insert(
        &self,
        world: &mut World,
        entity: EntityId,
        component: &SceneComponent,
    ) -> Result<(), SceneError> {
        let codec =
            self.codecs
                .get(&component.name)
                .ok_or_else(|| SceneError::UnknownComponent {
                    name: component.name.clone(),
                })?;
        (codec.insert)(world, entity, &component.value).map_err(|error| SceneError::Component {
            entity,
            name: component.name.clone(),
            message: error.to_string(),
        })
    }
}

/// A serialized `World`: entities and their registered components.
///
/// The file-facing structs are their own types rather than the engine's
/// (there are no `World` internals in here), so the format survives a
/// rewrite of `World` itself. `Scene` is `PartialEq` so tests and tooling can
/// compare parse-load-save round trips.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    /// [`Scene::VERSION`] when this build wrote it.
    pub version: u32,
    pub entities: Vec<SceneEntity>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SceneEntity {
    pub id: EntityId,
    pub components: Vec<SceneComponent>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SceneComponent {
    /// The name the component was registered under, saved verbatim.
    pub name: String,
    /// The component's payload as a single line of RON text.
    pub value: Box<RawValue>,
}

impl<'de> Deserialize<'de> for SceneComponent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct SceneComponentVisitor;

        impl<'de> serde::de::Visitor<'de> for SceneComponentVisitor {
            type Value = SceneComponent;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a `SceneComponent` with a `name` and a raw `value`")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut name = None;
                let mut value = None;
                while let Some(field) = map.next_key::<String>()? {
                    match field.as_str() {
                        "name" => name = Some(map.next_value()?),
                        "value" => value = Some(map.next_value::<Box<RawValue>>()?.trim_boxed()),
                        other => {
                            return Err(serde::de::Error::unknown_field(other, &["name", "value"]));
                        }
                    }
                }
                Ok(SceneComponent {
                    name: name.ok_or_else(|| serde::de::Error::missing_field("name"))?,
                    value: value.ok_or_else(|| serde::de::Error::missing_field("value"))?,
                })
            }
        }

        deserializer.deserialize_struct("SceneComponent", &["name", "value"], SceneComponentVisitor)
    }
}

/// A scene parsed into its components, ready for a `World`.
pub struct LoadedScene {
    /// The re-hydrated entities and components.
    pub world: World,
    /// What the registry could not hold — unknown component names, skipped on
    /// load and so absent on the next save. A caller logs these.
    pub warnings: Vec<String>,
}

impl Scene {
    /// The format version this build reads. Files with `version > VERSION`
    /// are refused as a future format; older files are still read.
    pub const VERSION: u32 = 1;

    /// Snapshot `world` through `registry` into a scene.
    ///
    /// Every codec is probed in registry name order (so output is stable) and
    /// every registered component an entity holds is saved under that name.
    /// Entities holding no registered component are dropped: an entity with
    /// nothing left is a deletion, which a save must not resurrect.
    pub fn from_world(world: &World, registry: &SceneRegistry) -> Result<Self, SceneError> {
        let mut entities = Vec::new();
        for id in world.entities() {
            let mut components = Vec::new();
            for (name, codec) in &registry.codecs {
                match (codec.encode)(world, id) {
                    None => {}
                    Some(Ok(value)) => components.push(SceneComponent {
                        name: name.clone(),
                        value,
                    }),
                    Some(Err(error)) => {
                        return Err(SceneError::Component {
                            entity: id,
                            name: name.clone(),
                            message: error.to_string(),
                        });
                    }
                }
            }
            entities.push(SceneEntity { id, components });
        }
        Ok(Self {
            version: Self::VERSION,
            entities,
        })
    }

    /// Load `world` back out of this scene.
    ///
    /// Components in the file but not in `registry` are skipped with a warning
    /// rather than failing the whole load — a forward-compatible read keeps
    /// what it knows. Load order is deliberate (entities ascend by id,
    /// components follow the file), so saving right after loading reproduces
    /// the same text.
    pub fn to_world(&self, registry: &SceneRegistry) -> Result<LoadedScene, SceneError> {
        self.check_version()?;
        let mut world = World::new();
        let mut warnings = Vec::new();
        let mut seen = BTreeSet::new();
        for entity in &self.entities {
            if !seen.insert(entity.id) {
                return Err(SceneError::DuplicateEntity(entity.id));
            }
            world.spawn_at(entity.id);
            for component in &entity.components {
                match registry.codecs.get(&component.name) {
                    Some(codec) => (codec.insert)(&mut world, entity.id, &component.value)
                        .map_err(|error| SceneError::Component {
                            entity: entity.id,
                            name: component.name.clone(),
                            message: error.to_string(),
                        })?,
                    None => warnings.push(format!(
                        "entity {}: unknown component `{}` — skipped",
                        entity.id, component.name
                    )),
                }
            }
        }
        Ok(LoadedScene { world, warnings })
    }

    fn check_version(&self) -> Result<(), SceneError> {
        match self.version {
            version if version <= Self::VERSION => Ok(()),
            version => Err(SceneError::Version {
                found: version,
                supported: Self::VERSION,
            }),
        }
    }

    /// The deterministic text form of this scene: pretty RON with LF
    /// newlines everywhere — a file must diff identically on every OS — and
    /// struct names on, so a scene reads as `Scene(...)` rather than `(...)`.
    pub fn to_text(&self) -> Result<String, SceneError> {
        let config = PrettyConfig::default().new_line("\n").struct_names(true);
        let mut text = Options::default()
            .to_string_pretty(self, config)
            .map_err(SceneError::Parse)?;
        text.push('\n');
        Ok(text)
    }

    /// Write [`to_text`](Scene::to_text) to `path`, creating missing
    /// directories along the way.
    pub fn save(&self, path: &Path) -> Result<(), SceneError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(SceneError::Io)?;
        }
        std::fs::write(path, self.to_text()?).map_err(SceneError::Io)
    }

    /// Read and parse a scene file.
    pub fn load(path: &Path) -> Result<Self, SceneError> {
        let text = std::fs::read_to_string(path).map_err(SceneError::Io)?;
        text.parse()
    }
}

impl FromStr for Scene {
    type Err = SceneError;

    fn from_str(text: &str) -> Result<Self, SceneError> {
        let scene: Scene = Options::default()
            .from_str(text)
            .map_err(|error| SceneError::Parse(RonError::from(error)))?;
        scene.check_version()?;
        Ok(scene)
    }
}

/// Why a scene file was refused, or could not be written.
#[derive(Debug)]
pub enum SceneError {
    /// The file could not be read or written.
    Io(std::io::Error),
    /// The file is not RON of the shape a scene takes.
    Parse(RonError),
    /// The file is a future format this build cannot read.
    Version { found: u32, supported: u32 },
    /// The file lists the same entity id twice.
    DuplicateEntity(EntityId),
    /// A component name has no codec in this registry.
    UnknownComponent { name: String },
    /// A component failed to encode or decode from/to its raw text.
    Component {
        entity: EntityId,
        name: String,
        message: String,
    },
}

impl fmt::Display for SceneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "scene file: {error}"),
            Self::Parse(error) => write!(f, "not valid scene text: {error}"),
            Self::Version { found, supported } => write!(
                f,
                "scene is format version {found}; this build reads up to version {supported}"
            ),
            Self::DuplicateEntity(id) => write!(f, "entity {id} appears more than once"),
            Self::UnknownComponent { name } => {
                write!(f, "component `{name}` is not registered for this project")
            }
            Self::Component {
                entity,
                name,
                message,
            } => write!(f, "entity {entity}: component `{name}` failed: {message}"),
        }
    }
}

impl std::error::Error for SceneError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Parse(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Vec2;

    fn registry() -> SceneRegistry {
        SceneRegistry::engine_defaults()
    }

    fn scene_with_two_entities() -> Scene {
        let mut world = World::new();
        let hero = world.spawn();
        let grind = world.spawn();
        world.insert(hero, Transform::from_position(Vec2::new(3.0, 4.0)));
        world.insert(
            grind,
            Transform::new(Vec2::new(10.0, 0.0), 0.5, Vec2::splat(2.0)),
        );
        Scene::from_world(&world, &registry()).unwrap()
    }

    #[test]
    fn empty_scene_roundtrips_through_its_text() {
        let scene = Scene {
            version: Scene::VERSION,
            entities: vec![],
        };
        let text = scene.to_text().unwrap();
        assert_eq!(text, "Scene(\n    version: 1,\n    entities: [],\n)\n");
        assert_eq!(text.parse::<Scene>().unwrap(), scene);
    }

    #[test]
    fn entities_and_transforms_save_and_reload_identically() {
        let scene = scene_with_two_entities();
        let back: Scene = scene.to_text().unwrap().parse().unwrap();
        assert_eq!(back, scene);

        let world = back.to_world(&registry()).unwrap().world;
        assert_eq!(
            world.get::<Transform>(0),
            Some(&Transform::from_position(Vec2::new(3.0, 4.0)))
        );
        assert_eq!(
            world.get::<Transform>(1),
            Some(&Transform::new(Vec2::new(10.0, 0.0), 0.5, Vec2::splat(2.0)))
        );
    }

    #[test]
    fn text_is_a_fixed_point_of_itself() {
        let text = scene_with_two_entities().to_text().unwrap();
        let again: Scene = text.parse().unwrap();
        assert_eq!(again.to_text().unwrap(), text);
    }

    #[test]
    fn unknown_components_are_skipped_with_a_warning() {
        let text = r#"Scene(
    version: 1,
    entities: [
        SceneEntity(
            id: 0,
            components: [
                SceneComponent(
                    name: "Transform",
                    value: (position: (1.0, 2.0), rotation: 0.0, scale: (1.0, 1.0)),
                ),
                SceneComponent(
                    name: "Loot",
                    value: 42,
                ),
            ],
        ),
    ],
)"#;
        let loaded = text
            .parse::<Scene>()
            .unwrap()
            .to_world(&registry())
            .unwrap();
        assert_eq!(
            loaded.warnings,
            vec!["entity 0: unknown component `Loot` — skipped".to_string()]
        );
        assert_eq!(
            loaded.world.get::<Transform>(0),
            Some(&Transform::from_position(Vec2::new(1.0, 2.0)))
        );
    }

    #[test]
    fn a_parent_link_saves_and_reloads() {
        let mut world = World::new();
        let body = world.spawn();
        let head = world.spawn();
        world.insert(body, Transform::default());
        world.insert(head, Transform::default());
        world.insert(head, Parent { entity: body });

        let scene = Scene::from_world(&world, &registry()).unwrap();
        let back: Scene = scene.to_text().unwrap().parse().unwrap();
        assert_eq!(back, scene);

        let world = back.to_world(&registry()).unwrap().world;
        assert_eq!(world.get::<Parent>(head), Some(&Parent { entity: body }));
    }

    #[test]
    fn a_snapshot_captures_what_an_entity_holds() {
        let mut world = World::new();
        let id = world.spawn();
        world.insert(id, Transform::from_position(Vec2::new(1.0, 2.0)));
        world.insert(id, Parent { entity: 7 });

        let components = registry().snapshot_entity(&world, id).unwrap();
        assert_eq!(
            components
                .iter()
                .map(|component| component.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Parent", "Transform"]
        );
    }

    #[test]
    fn inserting_an_unknown_component_is_refused() {
        let mut world = World::new();
        let id = world.spawn();
        let captured = SceneComponent {
            name: "Loot".to_owned(),
            value: RawValue::from_rust(&42_i32).unwrap(),
        };
        match registry().insert(&mut world, id, &captured) {
            Err(SceneError::UnknownComponent { name }) => assert_eq!(name, "Loot"),
            other => panic!("wrong result: {other:?}"),
        }
    }

    #[test]
    fn a_future_version_is_refused() {
        let error = "Scene(version: 99, entities: [])"
            .parse::<Scene>()
            .unwrap_err();
        match error {
            SceneError::Version {
                found: 99,
                supported: 1,
            } => {}
            other => panic!("wrong error: {other}"),
        }
    }

    #[test]
    fn a_duplicate_entity_id_is_refused() {
        let scene: Scene =
            "Scene(version: 1, entities: [SceneEntity(id: 0, components: []), SceneEntity(id: 0, components: [])])"
                .parse()
                .unwrap();
        assert!(matches!(
            scene.to_world(&registry()),
            Err(SceneError::DuplicateEntity(0))
        ));
    }

    #[test]
    fn garbage_text_is_reported_as_a_parse_error() {
        assert!(matches!(
            "not a scene".parse::<Scene>(),
            Err(SceneError::Parse(_))
        ));
    }
}

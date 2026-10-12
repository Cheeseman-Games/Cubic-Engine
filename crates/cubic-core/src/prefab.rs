//! Prefabs: a saved scene fragment that can be stamped back into a world.
//!
//! A prefab is a subtree lifted out of a scene and written to its own file, so
//! an object built once can be placed many times. It reuses the scene model
//! ([`SceneEntity`], [`SceneComponent`], [`SceneRegistry`]) rather than inventing
//! a second serialization: a prefab is a `Scene` with a named root and no
//! promise that its entity ids mean anything on their own.
//!
//! # Ids are local to the fragment
//!
//! [`capture`](Prefab::capture) keeps the source entity ids so the fragment's
//! internal `Parent` links stay valid — entity 1 still names entity 0 inside the
//! file. Only [`instantiate`](Prefab::instantiate) gives the fragment fresh ids
//! in the target world, remapping every `Parent` to match, which is what lets the
//! same prefab be placed twice without the two copies fighting over one id.
//!
//! A link that points *out* of the captured subtree (the root's own parent, say)
//! is dropped at capture time: a fragment that reached back into the scene it was
//! cut from could not stand alone.
//!
//! # Text format
//!
//! Exactly the scene format — pretty RON, LF newlines, one component per line —
//! with `Prefab` instead of `Scene` at the top, so it diffs the same way.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;
use std::str::FromStr;

use ron::Error as RonError;
use ron::options::Options;
use ron::ser::PrettyConfig;
use ron::value::RawValue;
use serde::{Deserialize, Serialize};

use crate::components::Transform;
use crate::hierarchy::{self, Parent};
use crate::math::Vec2;
use crate::scene::{SceneComponent, SceneEntity, SceneError, SceneRegistry};
use crate::world::{EntityId, World};

/// A reusable fragment of a scene: a root entity and everything under it.
///
/// `PartialEq` so a parse-load-save round trip can be compared, like [`Scene`].
///
/// [`Scene`]: crate::scene::Scene
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Prefab {
    /// [`Prefab::VERSION`] when this build wrote it.
    pub version: u32,
    /// The fragment entity the rest hangs from; the anchor
    /// [`instantiate`](Prefab::instantiate) moves to the drop point.
    pub root: EntityId,
    /// The fragment's entities, ascending by their local ids.
    pub entities: Vec<SceneEntity>,
}

/// What [`instantiate`](Prefab::instantiate) put into the world.
#[derive(Clone, Debug, PartialEq)]
pub struct Instantiated {
    /// The new entity the fragment's root became.
    pub root: EntityId,
    /// Every new entity, with the components it now holds — a snapshot the
    /// editor records so the whole placement can be undone as one step.
    pub entities: Vec<(EntityId, Vec<SceneComponent>)>,
}

impl Prefab {
    /// The format version this build reads. Files with `version > VERSION` are
    /// refused as a future format; older files are still read.
    pub const VERSION: u32 = 1;

    /// The file extension a prefab is saved under, without the dot.
    pub const EXTENSION: &'static str = "prefab";

    /// Lifts the subtree rooted at `root` out of `world`.
    ///
    /// Every entity under `root` is captured through `registry`, with its
    /// original id. Links that leave the subtree are dropped, so the fragment
    /// stands alone.
    pub fn capture(
        world: &World,
        registry: &SceneRegistry,
        root: EntityId,
    ) -> Result<Self, PrefabError> {
        if !world.entities().contains(&root) {
            return Err(PrefabError::NotInWorld(root));
        }
        let mut ids = vec![root];
        ids.extend(hierarchy::descendants(world, root));
        ids.sort_unstable();
        ids.dedup();
        let inside: BTreeSet<EntityId> = ids.iter().copied().collect();

        let mut entities = Vec::with_capacity(ids.len());
        for id in ids {
            entities.push(SceneEntity {
                id,
                components: registry.snapshot_entity(world, id)?,
            });
        }
        for entity in &mut entities {
            entity.components.retain(|component| {
                if component.name != "Parent" {
                    return true;
                }
                match component.value.into_rust::<Parent>() {
                    Ok(parent) => inside.contains(&parent.entity),
                    // A `Parent` this build cannot read is left alone rather than
                    // silently discarded.
                    Err(_) => true,
                }
            });
        }
        Ok(Self {
            version: Self::VERSION,
            root,
            entities,
        })
    }

    /// Stamps the fragment into `world`, giving every entity a fresh id.
    ///
    /// Every `Parent` is remapped onto the new ids, so the fragment keeps its
    /// shape. When `at` is given the whole fragment is placed so its root lands
    /// there — each transform shifts by `at - root position` — which is what a
    /// drop onto the viewport wants; a fragment whose root has no `Transform` is
    /// left where its own coordinates put it.
    ///
    /// The fragment is checked before anything is spawned: a duplicate id, a
    /// missing root, or a component no codec in `registry` knows is refused with
    /// the world untouched.
    pub fn instantiate(
        &self,
        world: &mut World,
        registry: &SceneRegistry,
        at: Option<Vec2>,
    ) -> Result<Instantiated, PrefabError> {
        self.check_version()?;

        let mut seen = BTreeSet::new();
        for entity in &self.entities {
            if !seen.insert(entity.id) {
                return Err(PrefabError::Scene(SceneError::DuplicateEntity(entity.id)));
            }
            for component in &entity.components {
                if !registry.has(&component.name) {
                    return Err(PrefabError::Scene(SceneError::UnknownComponent {
                        name: component.name.clone(),
                    }));
                }
            }
        }
        if !seen.contains(&self.root) {
            return Err(PrefabError::MissingRoot(self.root));
        }

        let shift = match at {
            Some(at) => at - self.root_position().unwrap_or(Vec2::ZERO),
            None => Vec2::ZERO,
        };

        let mut remap: BTreeMap<EntityId, EntityId> = BTreeMap::new();
        for entity in &self.entities {
            remap.insert(entity.id, world.spawn());
        }
        let root = remap[&self.root];

        for entity in &self.entities {
            let target = remap[&entity.id];
            for component in &entity.components {
                if let Some(component) = remap_component(component, &remap, shift)? {
                    registry
                        .insert(world, target, &component)
                        .map_err(PrefabError::Scene)?;
                }
            }
        }

        let mut entities = Vec::with_capacity(remap.len());
        for target in remap.values() {
            entities.push((*target, registry.snapshot_entity(world, *target)?));
        }
        Ok(Instantiated { root, entities })
    }

    /// The fragment root's world position, read off its saved `Transform`.
    fn root_position(&self) -> Option<Vec2> {
        let entity = self.entities.iter().find(|entity| entity.id == self.root)?;
        let component = entity
            .components
            .iter()
            .find(|component| component.name == "Transform")?;
        component
            .value
            .into_rust::<Transform>()
            .ok()
            .map(|transform| transform.position)
    }

    /// Refuses a version this build cannot read.
    pub fn check_version(&self) -> Result<(), PrefabError> {
        match self.version {
            version if version <= Self::VERSION => Ok(()),
            version => Err(PrefabError::Scene(SceneError::Version {
                found: version,
                supported: Self::VERSION,
            })),
        }
    }

    /// The deterministic text form of this prefab: pretty RON with LF newlines
    /// and struct names on, matching the scene format.
    pub fn to_text(&self) -> Result<String, PrefabError> {
        let config = PrettyConfig::default().new_line("\n").struct_names(true);
        let mut text = Options::default()
            .to_string_pretty(self, config)
            .map_err(|error| PrefabError::Scene(SceneError::Parse(error)))?;
        text.push('\n');
        Ok(text)
    }

    /// Write [`to_text`](Prefab::to_text) to `path`, creating missing
    /// directories along the way.
    pub fn save(&self, path: &Path) -> Result<(), PrefabError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| PrefabError::Scene(SceneError::Io(error)))?;
        }
        std::fs::write(path, self.to_text()?)
            .map_err(|error| PrefabError::Scene(SceneError::Io(error)))
    }

    /// Read and parse a prefab file.
    pub fn load(path: &Path) -> Result<Self, PrefabError> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| PrefabError::Scene(SceneError::Io(error)))?;
        text.parse()
    }
}

/// Rewrites the two components whose saved text names other entities or the
/// world: `Parent` (new ids) and `Transform` (the anchor shift).
///
/// Anything else is passed through untouched. A component named `Parent` that
/// does not decode as a [`Parent`] is left opaque rather than dropped.
fn remap_component(
    component: &SceneComponent,
    remap: &BTreeMap<EntityId, EntityId>,
    shift: Vec2,
) -> Result<Option<SceneComponent>, PrefabError> {
    match component.name.as_str() {
        "Parent" => {
            let Ok(parent) = component.value.into_rust::<Parent>() else {
                return Ok(Some(component.clone()));
            };
            let Some(&new) = remap.get(&parent.entity) else {
                // A link that leaves the fragment has no target in the new
                // world; dropping it keeps the fragment self-contained.
                return Ok(None);
            };
            Ok(Some(SceneComponent {
                name: component.name.clone(),
                value: encode(&Parent { entity: new })?,
            }))
        }
        "Transform" => {
            let Ok(transform) = component.value.into_rust::<Transform>() else {
                return Ok(Some(component.clone()));
            };
            if shift == Vec2::ZERO {
                return Ok(Some(component.clone()));
            }
            let moved = Transform {
                position: transform.position + shift,
                ..transform
            };
            Ok(Some(SceneComponent {
                name: component.name.clone(),
                value: encode(&moved)?,
            }))
        }
        _ => Ok(Some(component.clone())),
    }
}

/// Serialize one component back to its single-line [`RawValue`], the same
/// encoding the scene codecs write.
fn encode<T: Serialize>(value: &T) -> Result<Box<RawValue>, PrefabError> {
    RawValue::from_rust(value).map_err(|error| PrefabError::Scene(SceneError::Parse(error)))
}

impl FromStr for Prefab {
    type Err = PrefabError;

    fn from_str(text: &str) -> Result<Self, PrefabError> {
        let prefab: Prefab = Options::default()
            .from_str(text)
            .map_err(|error| PrefabError::Scene(SceneError::Parse(RonError::from(error))))?;
        prefab.check_version()?;
        Ok(prefab)
    }
}

/// Why a prefab was refused, or could not be placed.
#[derive(Debug)]
pub enum PrefabError {
    /// The scene layer refused an encode, decode, version, or id check.
    Scene(SceneError),
    /// The entity asked to be captured is not in the world.
    NotInWorld(EntityId),
    /// The fragment names a root that it does not contain.
    MissingRoot(EntityId),
}

impl fmt::Display for PrefabError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Scene(error) => write!(f, "{error}"),
            Self::NotInWorld(id) => write!(f, "entity #{id} is not in the world"),
            Self::MissingRoot(id) => {
                write!(f, "the prefab has no entity #{id} to serve as its root")
            }
        }
    }
}

impl std::error::Error for PrefabError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Scene(error) => Some(error),
            _ => None,
        }
    }
}

impl From<SceneError> for PrefabError {
    fn from(error: SceneError) -> Self {
        Self::Scene(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> SceneRegistry {
        SceneRegistry::engine_defaults()
    }

    /// A world with a small tree — 0 → 1 → 2 — and a detached entity 3.
    fn tree() -> World {
        let mut world = World::new();
        for _ in 0..4 {
            let id = world.spawn();
            world.insert(id, Transform::default());
        }
        world.insert(1, Parent { entity: 0 });
        world.insert(2, Parent { entity: 1 });
        world
    }

    /// The value of `entity`'s component registered under `name`, as text.
    fn component<'a>(prefab: &'a Prefab, id: EntityId, name: &str) -> Option<&'a SceneComponent> {
        prefab
            .entities
            .iter()
            .find(|entity| entity.id == id)?
            .components
            .iter()
            .find(|component| component.name == name)
    }

    #[test]
    fn capturing_a_subtree_keeps_only_the_subtree() {
        let mut world = tree();
        // The root names an outside parent; a fragment must not carry that link.
        world.insert(0, Parent { entity: 3 });

        let prefab = Prefab::capture(&world, &registry(), 0).unwrap();

        let ids: Vec<EntityId> = prefab.entities.iter().map(|entity| entity.id).collect();
        assert_eq!(ids, vec![0, 1, 2], "the detached entity is left behind");
        assert_eq!(prefab.root, 0);
        assert!(
            component(&prefab, 0, "Parent").is_none(),
            "the root's external parent is dropped"
        );
        assert_eq!(
            component(&prefab, 1, "Parent")
                .unwrap()
                .value
                .into_rust::<Parent>()
                .unwrap(),
            Parent { entity: 0 },
            "an internal link survives"
        );
    }

    #[test]
    fn capturing_an_entity_not_in_the_world_is_refused() {
        let world = tree();
        assert!(matches!(
            Prefab::capture(&world, &registry(), 99),
            Err(PrefabError::NotInWorld(99))
        ));
    }

    #[test]
    fn a_prefab_round_trips_through_its_text() {
        let world = tree();
        let prefab = Prefab::capture(&world, &registry(), 0).unwrap();
        let text = prefab.to_text().unwrap();
        let back: Prefab = text.parse().unwrap();
        assert_eq!(back, prefab);
    }

    #[test]
    fn text_is_a_fixed_point_of_itself() {
        let world = tree();
        let text = Prefab::capture(&world, &registry(), 0)
            .unwrap()
            .to_text()
            .unwrap();
        let again: Prefab = text.parse().unwrap();
        assert_eq!(again.to_text().unwrap(), text);
    }

    #[test]
    fn instantiating_remaps_parents_and_places_the_fragment_at_an_anchor() {
        let mut source = World::new();
        let root = source.spawn();
        let child = source.spawn();
        source.insert(root, Transform::from_position(Vec2::new(5.0, 5.0)));
        source.insert(child, Transform::from_position(Vec2::new(5.0, 10.0)));
        source.insert(child, Parent { entity: root });
        let prefab = Prefab::capture(&source, &registry(), root).unwrap();

        let mut target = World::new();
        let existing = target.spawn();
        target.insert(existing, Transform::from_position(Vec2::new(-100.0, 0.0)));

        let at = Vec2::new(3.0, 4.0);
        let placed = prefab
            .instantiate(&mut target, &registry(), Some(at))
            .unwrap();

        assert_eq!(placed.root, 1, "the fresh id after the existing entity");
        assert_eq!(placed.entities.len(), 2);
        assert_eq!(
            target.get::<Transform>(placed.root).unwrap().position,
            at,
            "the root lands on the anchor"
        );
        assert_eq!(
            target.get::<Transform>(2).unwrap().position,
            Vec2::new(3.0, 9.0),
            "the child moves with the root"
        );
        assert_eq!(
            hierarchy::parent_of(&target, 2),
            Some(1),
            "the parent link points at the new id"
        );
        assert_eq!(
            target.get::<Transform>(existing).unwrap().position,
            Vec2::new(-100.0, 0.0),
            "the entity already there is untouched"
        );
    }

    #[test]
    fn instantiation_without_an_anchor_keeps_the_fragment_in_place() {
        let mut source = World::new();
        let root = source.spawn();
        source.insert(root, Transform::from_position(Vec2::new(5.0, 6.0)));
        let prefab = Prefab::capture(&source, &registry(), root).unwrap();

        let mut target = World::new();
        let placed = prefab.instantiate(&mut target, &registry(), None).unwrap();

        assert_eq!(
            target.get::<Transform>(placed.root).unwrap().position,
            Vec2::new(5.0, 6.0)
        );
    }

    #[test]
    fn a_fragment_without_a_transform_root_is_not_moved() {
        let mut source = World::new();
        let root = source.spawn();
        source.insert(root, Parent { entity: 9 });
        let prefab = Prefab::capture(&source, &registry(), root).unwrap();
        // The only component was an external parent, dropped: the fragment root
        // holds nothing, so an anchor has nothing to shift.
        assert!(prefab.entities[0].components.is_empty());

        let mut target = World::new();
        let placed = prefab
            .instantiate(&mut target, &registry(), Some(Vec2::new(1.0, 2.0)))
            .unwrap();
        assert_eq!(placed.entities.len(), 1);
    }

    #[test]
    fn a_second_instantiation_takes_fresh_ids() {
        let world = tree();
        let prefab = Prefab::capture(&world, &registry(), 0).unwrap();

        let mut target = World::new();
        let first = prefab.instantiate(&mut target, &registry(), None).unwrap();
        let second = prefab.instantiate(&mut target, &registry(), None).unwrap();

        let first_ids: BTreeSet<EntityId> = first.entities.iter().map(|(id, _)| *id).collect();
        let second_ids: BTreeSet<EntityId> = second.entities.iter().map(|(id, _)| *id).collect();
        assert!(
            first_ids.is_disjoint(&second_ids),
            "two copies must not share an id"
        );
        assert_eq!(first.root, 0);
        assert_eq!(second.root, 3);
    }

    #[test]
    fn a_future_version_is_refused() {
        let error = "Prefab(version: 99, root: 0, entities: [])"
            .parse::<Prefab>()
            .unwrap_err();
        assert!(matches!(
            error,
            PrefabError::Scene(SceneError::Version {
                found: 99,
                supported: 1,
            })
        ));
    }

    #[test]
    fn a_prefab_without_its_root_is_refused() {
        let prefab = Prefab {
            version: Prefab::VERSION,
            root: 9,
            entities: vec![SceneEntity {
                id: 0,
                components: vec![],
            }],
        };
        assert!(matches!(
            prefab.instantiate(&mut World::new(), &registry(), None),
            Err(PrefabError::MissingRoot(9))
        ));
    }

    #[test]
    fn a_duplicate_entity_id_is_refused() {
        let prefab = Prefab {
            version: Prefab::VERSION,
            root: 0,
            entities: vec![
                SceneEntity {
                    id: 0,
                    components: vec![],
                },
                SceneEntity {
                    id: 0,
                    components: vec![],
                },
            ],
        };
        assert!(matches!(
            prefab.instantiate(&mut World::new(), &registry(), None),
            Err(PrefabError::Scene(SceneError::DuplicateEntity(0)))
        ));
    }

    #[test]
    fn an_unknown_component_is_refused() {
        let prefab = Prefab {
            version: Prefab::VERSION,
            root: 0,
            entities: vec![SceneEntity {
                id: 0,
                components: vec![SceneComponent {
                    name: "Loot".to_owned(),
                    value: RawValue::from_rust(&42_i32).unwrap(),
                }],
            }],
        };
        match prefab.instantiate(&mut World::new(), &registry(), None) {
            Err(PrefabError::Scene(SceneError::UnknownComponent { name })) => {
                assert_eq!(name, "Loot");
            }
            other => panic!("wrong result: {other:?}"),
        }
    }

    #[test]
    fn garbage_text_is_reported_as_a_parse_error() {
        assert!(matches!(
            "not a prefab".parse::<Prefab>(),
            Err(PrefabError::Scene(SceneError::Parse(_)))
        ));
    }
}

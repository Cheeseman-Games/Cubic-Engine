//! The edit command queue: the one path every scene change takes.
//!
//! A builder here validates a request against the open scene, performs it, and
//! remembers how to take it back, so panels never touch a [`World`] directly.
//! That single funnel is what makes undo possible: a change that went through
//! a command can be reversed, and the panel that asked for it never had to
//! know how.
//!
//! # Reversal is by components, never by ids
//!
//! [`World::despawn`] recycles entity ids, and a recycled id would alias
//! whatever command still referred to the old entity — delete an entity, add
//! one that reuses the id, undo the delete, and two commands would fight over
//! one slot. Commands therefore reach the [`SceneRegistry`] (the same codecs a
//! `.rsn` file uses) to capture an entity as text and to put it back, and a
//! delete *empties* an entity of its components instead of despawning it. An
//! empty id is invisible to `World::entities` and to a save, so it *is* a
//! deletion — but its id is never handed out again. The editor's world only
//! ever holds components the registry knows (a loaded scene skips the rest),
//! so emptying is complete.

use cubic_core::components::Transform;
use cubic_core::hierarchy::{self, Parent};
use cubic_core::reflect::ComponentRegistry;
use cubic_core::scene::{SceneComponent, SceneRegistry};
use cubic_core::world::{EntityId, World};

use crate::state::{EditorState, Selection};

/// How many changes are remembered before the oldest is forgotten.
const CAPACITY: usize = 256;

/// The registries and world a command acts on.
pub struct EditContext<'a> {
    pub world: &'a mut World,
    pub scenes: &'a SceneRegistry,
    pub components: &'a ComponentRegistry,
}

/// One reversible change to the open scene.
///
/// Every variant carries what its own reversal needs, so apply and rollback
/// are both local: nothing is looked up from the world at undo time that could
/// have moved since the change was made.
#[derive(Clone, Debug)]
pub enum EditCommand {
    /// An entity came into being, holding `components`.
    Spawn {
        id: EntityId,
        components: Vec<SceneComponent>,
    },
    /// An entity was deleted, having held `components`.
    Despawn {
        id: EntityId,
        components: Vec<SceneComponent>,
    },
    /// A component was added to an entity.
    AddComponent {
        entity: EntityId,
        name: &'static str,
    },
    /// A component was taken off an entity.
    RemoveComponent {
        entity: EntityId,
        component: SceneComponent,
    },
    /// An entity's parent link changed.
    Reparent {
        entity: EntityId,
        parent: Option<EntityId>,
        previous: Option<EntityId>,
    },
    /// Changes that happen and come back together, in order.
    Batch(Vec<EditCommand>),
}

impl EditCommand {
    /// Replays the change, as redo does.
    pub fn apply(&self, ctx: &mut EditContext<'_>) -> Result<(), String> {
        match self {
            Self::Spawn { id, components } => insert_all(ctx, *id, components),
            Self::Despawn { id, components } => remove_all(ctx, *id, components),
            Self::AddComponent { entity, name } => add_default(ctx, *entity, name),
            Self::RemoveComponent { entity, component } => {
                remove_one(ctx, *entity, component.name.as_str())
            }
            Self::Reparent { entity, parent, .. } => set_parent(ctx.world, *entity, *parent),
            Self::Batch(commands) => {
                for (index, command) in commands.iter().enumerate() {
                    if let Err(error) = command.apply(ctx) {
                        for undone in commands[..index].iter().rev() {
                            let _ = undone.rollback(ctx);
                        }
                        return Err(error);
                    }
                }
                Ok(())
            }
        }
    }

    /// Reverses the change, as undo does.
    pub fn rollback(&self, ctx: &mut EditContext<'_>) -> Result<(), String> {
        match self {
            Self::Spawn { id, components } => remove_all(ctx, *id, components),
            Self::Despawn { id, components } => insert_all(ctx, *id, components),
            Self::AddComponent { entity, name } => remove_one(ctx, *entity, name),
            Self::RemoveComponent { entity, component } => insert_one(ctx, *entity, component),
            Self::Reparent {
                entity, previous, ..
            } => set_parent(ctx.world, *entity, *previous),
            Self::Batch(commands) => {
                for (index, command) in commands.iter().rev().enumerate() {
                    if let Err(error) = command.rollback(ctx) {
                        for redone in commands.iter().rev().take(index).rev() {
                            let _ = redone.apply(ctx);
                        }
                        return Err(error);
                    }
                }
                Ok(())
            }
        }
    }

    /// A phrase for the console, naming what the command changes.
    pub fn label(&self) -> String {
        match self {
            Self::Spawn { id, .. } => format!("create entity #{id}"),
            Self::Despawn { id, .. } => format!("delete entity #{id}"),
            Self::AddComponent { entity, name } => format!("add {name} to entity #{entity}"),
            Self::RemoveComponent { entity, component } => {
                format!("remove {} from entity #{entity}", component.name)
            }
            Self::Reparent { entity, .. } => format!("reparent entity #{entity}"),
            Self::Batch(commands) => match commands.first() {
                Some(Self::Despawn { id, .. }) => format!("delete entity #{id} and its subtree"),
                _ => format!("apply {} changes", commands.len()),
            },
        }
    }
}

/// Inserts every captured component into `id`.
fn insert_all(
    ctx: &mut EditContext<'_>,
    id: EntityId,
    components: &[SceneComponent],
) -> Result<(), String> {
    for component in components {
        insert_one(ctx, id, component)?;
    }
    Ok(())
}

/// Inserts one captured component into `id`.
fn insert_one(
    ctx: &mut EditContext<'_>,
    id: EntityId,
    component: &SceneComponent,
) -> Result<(), String> {
    ctx.scenes
        .insert(ctx.world, id, component)
        .map_err(|error| error.to_string())
}

/// Takes every named component off `id`.
fn remove_all(
    ctx: &mut EditContext<'_>,
    id: EntityId,
    components: &[SceneComponent],
) -> Result<(), String> {
    for component in components {
        remove_one(ctx, id, component.name.as_str())?;
    }
    Ok(())
}

/// Takes the component registered under `name` off `id`.
fn remove_one(ctx: &mut EditContext<'_>, id: EntityId, name: &str) -> Result<(), String> {
    let entry = ctx
        .components
        .get(name)
        .ok_or_else(|| format!("component `{name}` is not registered"))?;
    entry.remove(ctx.world, id);
    Ok(())
}

/// Gives `id` a fresh default of the component registered under `name`.
fn add_default(ctx: &mut EditContext<'_>, id: EntityId, name: &str) -> Result<(), String> {
    let entry = ctx
        .components
        .get(name)
        .ok_or_else(|| format!("component `{name}` is not registered"))?;
    if !entry.add_default(ctx.world, id) {
        return Err(format!("component `{name}` cannot be added from scratch"));
    }
    Ok(())
}

/// Writes or clears an entity's parent link.
fn set_parent(world: &mut World, id: EntityId, parent: Option<EntityId>) -> Result<(), String> {
    match parent {
        Some(parent) => {
            world.insert(id, Parent { entity: parent });
        }
        None => {
            world.remove::<Parent>(id);
        }
    }
    Ok(())
}

/// The undo and redo stacks, oldest first.
#[derive(Default)]
pub struct EditHistory {
    undo: Vec<EditCommand>,
    redo: Vec<EditCommand>,
}

impl EditHistory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Remembers `command`, discarding any redo branch.
    fn record(&mut self, command: EditCommand) {
        self.redo.clear();
        self.undo.push(command);
        if self.undo.len() > CAPACITY {
            self.undo.remove(0);
        }
    }

    /// Forgets everything, for when the world underneath is replaced.
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// How many changes can be undone and redone, for a status readout.
    pub fn depth(&self) -> (usize, usize) {
        (self.undo.len(), self.redo.len())
    }

    fn pop_undo(&mut self) -> Option<EditCommand> {
        self.undo.pop()
    }

    fn pop_redo(&mut self) -> Option<EditCommand> {
        self.redo.pop()
    }

    fn push_undo(&mut self, command: EditCommand) {
        self.undo.push(command);
    }

    fn push_redo(&mut self, command: EditCommand) {
        self.redo.push(command);
    }
}

/// Runs `change` with the open scene's world and the registries, or reports
/// that no scene is open.
fn with_context<R>(
    state: &mut EditorState,
    change: impl FnOnce(&mut EditContext<'_>) -> Result<R, String>,
) -> Result<R, String> {
    let open = state
        .scene
        .as_mut()
        .ok_or_else(|| "no scene is open".to_owned())?;
    let mut ctx = EditContext {
        world: &mut open.world,
        scenes: &state.registry,
        components: &state.components,
    };
    change(&mut ctx)
}

/// Applies a command to the live world, without recording it.
fn apply_now(state: &mut EditorState, command: &EditCommand) -> Result<(), String> {
    with_context(state, |ctx| command.apply(ctx))
}

/// Marks the scene dirty and drops a selection that no longer exists.
fn touched(state: &mut EditorState) {
    state.dirty = true;
    if let Selection::Entity(id) = state.selection
        && let Some(open) = &state.scene
        && !open.world.entities().contains(&(id as EntityId))
    {
        state.selection = Selection::None;
    }
}

/// Adds a new entity, optionally under `parent`, and selects it.
pub fn add_entity(state: &mut EditorState, parent: Option<EntityId>) -> Result<String, String> {
    let (id, command) = {
        let open = state
            .scene
            .as_mut()
            .ok_or_else(|| "no scene is open".to_owned())?;
        if let Some(parent) = parent
            && !open.world.entities().contains(&parent)
        {
            return Err(format!("entity #{parent} is not in the scene"));
        }
        let id = open.world.spawn();
        open.world.insert(id, Transform::default());
        if let Some(parent) = parent {
            open.world.insert(id, Parent { entity: parent });
        }
        let components = state
            .registry
            .snapshot_entity(&open.world, id)
            .map_err(|error| error.to_string())?;
        (id, EditCommand::Spawn { id, components })
    };

    state.edits.record(command);
    state.dirty = true;
    state.selection = Selection::Entity(id as u64);
    Ok(match parent {
        Some(parent) => format!("added entity #{id} under #{parent}"),
        None => format!("added entity #{id}"),
    })
}

/// Deletes an entity and everything under it.
pub fn delete_entity(state: &mut EditorState, id: EntityId) -> Result<String, String> {
    let (command, descendants) = {
        let open = state
            .scene
            .as_mut()
            .ok_or_else(|| "no scene is open".to_owned())?;
        if !open.world.entities().contains(&id) {
            return Err(format!("entity #{id} is not in the scene"));
        }
        let mut targets = vec![id];
        targets.extend(hierarchy::descendants(&open.world, id));
        let descendants = targets.len() - 1;
        let mut commands = Vec::new();
        for target in targets {
            let components = state
                .registry
                .snapshot_entity(&open.world, target)
                .map_err(|error| error.to_string())?;
            commands.push(EditCommand::Despawn {
                id: target,
                components,
            });
        }
        let command = if commands.len() == 1 {
            commands.pop().expect("one command")
        } else {
            EditCommand::Batch(commands)
        };
        (command, descendants)
    };

    apply_now(state, &command)?;
    state.edits.record(command);
    touched(state);
    Ok(match descendants {
        0 => format!("deleted entity #{id}"),
        count => format!("deleted entity #{id} and {count} descendant(s)"),
    })
}

/// Copies an entity's components onto a new entity — a sibling of the source,
/// because the copy keeps whatever parent the source had.
pub fn duplicate_entity(state: &mut EditorState, id: EntityId) -> Result<String, String> {
    let (new_id, command) = {
        let open = state
            .scene
            .as_mut()
            .ok_or_else(|| "no scene is open".to_owned())?;
        if !open.world.entities().contains(&id) {
            return Err(format!("entity #{id} is not in the scene"));
        }
        let components = state
            .registry
            .snapshot_entity(&open.world, id)
            .map_err(|error| error.to_string())?;
        let new_id = open.world.spawn();
        (
            new_id,
            EditCommand::Spawn {
                id: new_id,
                components,
            },
        )
    };

    apply_now(state, &command)?;
    state.edits.record(command);
    state.dirty = true;
    state.selection = Selection::Entity(new_id as u64);
    Ok(format!("duplicated entity #{id} as #{new_id}"))
}

/// Adds a component to an entity from the add menu.
pub fn add_component(
    state: &mut EditorState,
    entity: EntityId,
    name: &'static str,
) -> Result<String, String> {
    {
        let open = state
            .scene
            .as_mut()
            .ok_or_else(|| "no scene is open".to_owned())?;
        if !open.world.entities().contains(&entity) {
            return Err(format!("entity #{entity} is not in the scene"));
        }
        if !state.registry.has(name) {
            return Err(format!(
                "`{name}` is not saved by this project's scene format"
            ));
        }
        let Some(entry) = state.components.get(name) else {
            return Err(format!("`{name}` is not a known component"));
        };
        if !entry.is_addable() {
            return Err(format!("`{name}` cannot be added from a menu"));
        }
    }

    let command = EditCommand::AddComponent { entity, name };
    apply_now(state, &command)?;
    state.edits.record(command);
    state.dirty = true;
    Ok(format!("added {name} to entity #{entity}"))
}

/// Takes a component off an entity, refusing to strip its last one.
pub fn remove_component(
    state: &mut EditorState,
    entity: EntityId,
    name: &'static str,
) -> Result<String, String> {
    let command = {
        let open = state
            .scene
            .as_mut()
            .ok_or_else(|| "no scene is open".to_owned())?;
        if !open.world.entities().contains(&entity) {
            return Err(format!("entity #{entity} is not in the scene"));
        }
        let Some(entry) = state.components.get(name) else {
            return Err(format!("`{name}` is not a known component"));
        };
        if !entry.has(&open.world, entity) {
            return Err(format!("entity #{entity} has no {name}"));
        }
        if state.components.on_entity(&open.world, entity).len() == 1 {
            return Err(format!(
                "{name} is entity #{entity}'s last component — delete the entity instead"
            ));
        }
        let component = match state.registry.encode(&open.world, entity, name) {
            Ok(Some(component)) => component,
            Ok(None) => {
                return Err(format!(
                    "`{name}` is not saved by this project's scene format, so removing \
                     it could not be undone"
                ));
            }
            Err(error) => return Err(error.to_string()),
        };
        EditCommand::RemoveComponent { entity, component }
    };

    apply_now(state, &command)?;
    state.edits.record(command);
    touched(state);
    Ok(format!("removed {name} from entity #{entity}"))
}

/// Points an entity at a new parent, or clears its parent.
pub fn reparent(
    state: &mut EditorState,
    entity: EntityId,
    parent: Option<EntityId>,
) -> Result<String, String> {
    let command = {
        let open = state
            .scene
            .as_mut()
            .ok_or_else(|| "no scene is open".to_owned())?;
        if !open.world.entities().contains(&entity) {
            return Err(format!("entity #{entity} is not in the scene"));
        }
        if let Some(parent) = parent {
            if parent == entity {
                return Err("an entity cannot be its own parent".to_owned());
            }
            if !open.world.entities().contains(&parent) {
                return Err(format!("entity #{parent} is not in the scene"));
            }
            if hierarchy::is_ancestor(&open.world, entity, parent) {
                return Err(format!("entity #{parent} is already under #{entity}"));
            }
        }
        let previous = hierarchy::parent_of(&open.world, entity);
        if previous == parent {
            return Ok(match parent {
                Some(parent) => format!("entity #{entity} is already under #{parent}"),
                None => format!("entity #{entity} has no parent"),
            });
        }
        EditCommand::Reparent {
            entity,
            parent,
            previous,
        }
    };

    apply_now(state, &command)?;
    state.edits.record(command);
    state.dirty = true;
    Ok(match parent {
        Some(parent) => format!("parented entity #{entity} to #{parent}"),
        None => format!("unparented entity #{entity}"),
    })
}

/// Reverses the most recent change.
pub fn undo(state: &mut EditorState) -> Result<String, String> {
    let Some(command) = state.edits.pop_undo() else {
        return Err("nothing to undo".to_owned());
    };
    let label = command.label();
    match with_context(state, |ctx| command.rollback(ctx)) {
        Ok(()) => {
            state.edits.push_redo(command);
            touched(state);
            Ok(format!("undid {label}"))
        }
        Err(error) => {
            state.edits.push_undo(command);
            Err(error)
        }
    }
}

/// Replays the most recently undone change.
pub fn redo(state: &mut EditorState) -> Result<String, String> {
    let Some(command) = state.edits.pop_redo() else {
        return Err("nothing to redo".to_owned());
    };
    let label = command.label();
    match with_context(state, |ctx| command.apply(ctx)) {
        Ok(()) => {
            state.edits.push_undo(command);
            touched(state);
            Ok(format!("redid {label}"))
        }
        Err(error) => {
            state.edits.push_redo(command);
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::state::OpenScene;

    fn state_with_scene() -> EditorState {
        EditorState {
            scene: Some(OpenScene {
                path: PathBuf::from("main.rsn"),
                world: World::new(),
            }),
            ..EditorState::default()
        }
    }

    fn entities(state: &EditorState) -> Vec<EntityId> {
        state.scene.as_ref().expect("scene").world.entities()
    }

    #[test]
    fn adding_an_entity_is_reversible() {
        let mut state = state_with_scene();
        assert_eq!(add_entity(&mut state, None).unwrap(), "added entity #0");
        assert_eq!(entities(&state), vec![0]);
        assert_eq!(state.selection, Selection::Entity(0));
        assert!(state.dirty);

        undo(&mut state).unwrap();
        assert!(entities(&state).is_empty());
        assert_eq!(
            state.selection,
            Selection::None,
            "a vanished selection is dropped"
        );

        redo(&mut state).unwrap();
        assert_eq!(entities(&state), vec![0]);
    }

    #[test]
    fn a_parented_entity_records_its_link() {
        let mut state = state_with_scene();
        add_entity(&mut state, None).unwrap();
        assert_eq!(
            add_entity(&mut state, Some(0)).unwrap(),
            "added entity #1 under #0"
        );
        let world = &state.scene.as_ref().unwrap().world;
        assert_eq!(hierarchy::parent_of(world, 1), Some(0));

        assert!(
            add_entity(&mut state, Some(9)).is_err(),
            "an unknown parent is refused"
        );
    }

    #[test]
    fn deleting_an_entity_takes_its_subtree_and_undo_brings_it_back() {
        let mut state = state_with_scene();
        add_entity(&mut state, None).unwrap();
        add_entity(&mut state, None).unwrap();
        add_entity(&mut state, Some(0)).unwrap();
        add_entity(&mut state, Some(2)).unwrap();

        assert_eq!(
            delete_entity(&mut state, 0).unwrap(),
            "deleted entity #0 and 2 descendant(s)"
        );
        assert_eq!(entities(&state), vec![1]);

        undo(&mut state).unwrap();
        let world = &state.scene.as_ref().unwrap().world;
        assert_eq!(world.entities(), vec![0, 1, 2, 3]);
        assert_eq!(hierarchy::parent_of(world, 2), Some(0));
        assert_eq!(hierarchy::parent_of(world, 3), Some(2));
    }

    #[test]
    fn duplicating_an_entity_repeats_its_components_as_a_sibling() {
        let mut state = state_with_scene();
        add_entity(&mut state, None).unwrap();
        add_entity(&mut state, Some(0)).unwrap();
        assert_eq!(
            duplicate_entity(&mut state, 1).unwrap(),
            "duplicated entity #1 as #2"
        );
        let world = &state.scene.as_ref().unwrap().world;
        assert_eq!(world.get::<Transform>(2), Some(&Transform::default()));
        assert_eq!(
            hierarchy::parent_of(world, 2),
            Some(0),
            "the copy shares its source's parent"
        );
        assert_eq!(state.selection, Selection::Entity(2));
    }

    #[test]
    fn removing_the_last_component_is_refused() {
        let mut state = state_with_scene();
        add_entity(&mut state, None).unwrap();
        let error = remove_component(&mut state, 0, "Transform").unwrap_err();
        assert!(error.contains("last component"), "got: {error}");
    }

    #[test]
    fn dropping_and_re_adding_a_component_round_trips() {
        let mut state = state_with_scene();
        add_entity(&mut state, None).unwrap();
        add_entity(&mut state, None).unwrap();
        reparent(&mut state, 0, Some(1)).unwrap();

        remove_component(&mut state, 0, "Transform").unwrap();
        assert_eq!(
            state.scene.as_ref().unwrap().world.get::<Transform>(0),
            None
        );

        add_component(&mut state, 0, "Transform").unwrap();
        assert_eq!(
            state.scene.as_ref().unwrap().world.get::<Transform>(0),
            Some(&Transform::default())
        );

        undo(&mut state).unwrap();
        assert_eq!(
            state.scene.as_ref().unwrap().world.get::<Transform>(0),
            None
        );
        undo(&mut state).unwrap();
        assert!(
            state
                .scene
                .as_ref()
                .unwrap()
                .world
                .get::<Transform>(0)
                .is_some()
        );
    }

    #[test]
    fn reparenting_refuses_cycles_and_self() {
        let mut state = state_with_scene();
        add_entity(&mut state, None).unwrap();
        add_entity(&mut state, Some(0)).unwrap();

        assert!(
            reparent(&mut state, 0, Some(0))
                .unwrap_err()
                .contains("own parent")
        );
        assert!(
            reparent(&mut state, 0, Some(1))
                .unwrap_err()
                .contains("already under")
        );
        assert_eq!(
            reparent(&mut state, 1, Some(0)).unwrap(),
            "entity #1 is already under #0"
        );
    }

    #[test]
    fn reparenting_is_reversible() {
        let mut state = state_with_scene();
        add_entity(&mut state, None).unwrap();
        add_entity(&mut state, None).unwrap();
        reparent(&mut state, 1, Some(0)).unwrap();
        assert_eq!(
            hierarchy::parent_of(&state.scene.as_ref().unwrap().world, 1),
            Some(0)
        );

        undo(&mut state).unwrap();
        assert_eq!(
            hierarchy::parent_of(&state.scene.as_ref().unwrap().world, 1),
            None
        );
    }

    #[test]
    fn a_deleted_id_is_not_handed_out_again() {
        let mut state = state_with_scene();
        add_entity(&mut state, None).unwrap();
        delete_entity(&mut state, 0).unwrap();
        add_entity(&mut state, None).unwrap();
        assert_eq!(
            state.selection,
            Selection::Entity(1),
            "the deleted id is never recycled"
        );

        undo(&mut state).unwrap();
        undo(&mut state).unwrap();
        assert!(
            entities(&state).contains(&0),
            "undoing the delete brings the same id back, with nothing to alias"
        );
    }

    #[test]
    fn undo_without_a_history_and_without_a_scene_report_cleanly() {
        let mut state = state_with_scene();
        assert_eq!(undo(&mut state).unwrap_err(), "nothing to undo");
        assert_eq!(redo(&mut state).unwrap_err(), "nothing to redo");

        let mut closed = EditorState::default();
        assert_eq!(
            add_entity(&mut closed, None).unwrap_err(),
            "no scene is open"
        );
    }

    #[test]
    fn a_new_change_discards_the_redo_branch() {
        let mut state = state_with_scene();
        add_entity(&mut state, None).unwrap();
        add_entity(&mut state, None).unwrap();
        undo(&mut state).unwrap();
        assert!(state.edits.can_redo());

        add_entity(&mut state, None).unwrap();
        assert!(
            !state.edits.can_redo(),
            "recording a change drops the redo stack"
        );
    }
}

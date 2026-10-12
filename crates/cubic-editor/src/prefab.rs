//! Placing and saving prefabs from the editor.
//!
//! The editor's bridge to [`cubic_core::prefab`]. Dropping a prefab from the
//! asset panel onto the viewport loads it and stamps it into the open scene
//! through the edit queue, so the whole placement is one undo step; the asset
//! panel's save button turns the current selection back into a prefab file.

use std::path::Path;

use cubic_core::math::Vec2;
use cubic_core::prefab::Prefab;
use cubic_core::world::EntityId;

use crate::assets::AssetCatalog;
use crate::edit::EditCommand;
use crate::state::{EditorState, Selection};

/// Where a project keeps its prefabs, relative to its root.
pub const DIR: &str = "assets/prefabs";

/// What a prefab row carries while it is dragged onto the viewport.
///
/// `Any + Send + Sync` is the shape egui's drag-and-drop stores payloads in.
#[derive(Clone, Debug)]
pub struct PrefabDrag {
    /// The prefab's path, relative to the project root.
    pub path: String,
    /// Its display name, for the console.
    pub name: String,
}

/// Loads the prefab at project-relative `path` and stamps it into the open
/// scene, recording the placement as a single undo step.
///
/// `at` is the world point the fragment's root should land on — where the
/// pointer released, for a drop — or `None` to keep the prefab's own
/// coordinates.
pub fn instantiate(
    state: &mut EditorState,
    path: &str,
    at: Option<Vec2>,
) -> Result<String, String> {
    let name = name_of(path);
    let root = state
        .project_root()
        .map(Path::to_path_buf)
        .ok_or_else(|| "no project is open — there are no prefabs to place".to_owned())?;
    let absolute = root.join(path);
    let prefab = Prefab::load(&absolute).map_err(|error| {
        format!(
            "could not load {}: {error}",
            crate::project::shown(state, &absolute)
        )
    })?;

    let (command, entity) = {
        // Split the fields apart so the world can be borrowed mutably while the
        // registry is read.
        let EditorState {
            scene, registry, ..
        } = state;
        let open = scene
            .as_mut()
            .ok_or_else(|| "no scene is open".to_owned())?;
        let placed = prefab
            .instantiate(&mut open.world, registry, at)
            .map_err(|error| error.to_string())?;
        let mut commands: Vec<EditCommand> = placed
            .entities
            .iter()
            .map(|(id, components)| EditCommand::Spawn {
                id: *id,
                components: components.clone(),
            })
            .collect();
        let command = if commands.len() == 1 {
            commands.pop().expect("one command")
        } else {
            EditCommand::Batch(commands)
        };
        (command, placed.root)
    };

    state.edits.record(command);
    state.dirty = true;
    state.selection = Selection::Entity(entity as u64);
    Ok(format!("instantiated `{name}` as #{entity}"))
}

/// Saves the selected entity's subtree as `<project>/assets/prefabs/<name>.prefab`
/// and rescans the catalog so the new file is listed right away.
pub fn save_selection(state: &mut EditorState, name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("give the prefab a name first".to_owned());
    }
    if name.contains(['/', '\\']) || name == "." || name == ".." {
        return Err(format!("`{name}` is not a usable file name"));
    }
    let root = state
        .project_root()
        .map(Path::to_path_buf)
        .ok_or_else(|| "no project is open — there is nowhere to save a prefab".to_owned())?;
    let entity = match state.selection {
        Selection::Entity(id) => id as EntityId,
        _ => return Err("select an entity to save as a prefab".to_owned()),
    };

    let prefab = {
        let open = state
            .scene
            .as_ref()
            .ok_or_else(|| "no scene is open".to_owned())?;
        if !open.world.entities().contains(&entity) {
            return Err(format!("entity #{entity} is not in the scene"));
        }
        Prefab::capture(&open.world, &state.registry, entity).map_err(|error| error.to_string())?
    };

    let relative = format!("{DIR}/{name}.{}", Prefab::EXTENSION);
    let path = root.join(&relative);
    prefab
        .save(&path)
        .map_err(|error| format!("could not save {relative}: {error}"))?;
    state.catalog = AssetCatalog::scan(&root);
    Ok(format!(
        "saved {relative} ({} entities)",
        prefab.entities.len()
    ))
}

/// A path's file stem, for naming a prefab in the panel and the console:
/// `assets/prefabs/x.prefab` reads as `x`.
pub(crate) fn name_of(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project;
    use crate::scratch::Scratch;
    use crate::state::OpenScene;
    use cubic_core::components::Transform;
    use cubic_core::hierarchy::Parent;
    use cubic_core::world::World;

    fn write_manifest(root: &Path) {
        std::fs::create_dir_all(root).expect("project dir");
        std::fs::write(root.join("game.toml"), "[game]\nname = \"demo\"\n").expect("manifest");
    }

    /// A state with an open project and an empty scene to build in.
    fn open(root: &Path) -> EditorState {
        let mut state = EditorState::new();
        project::open_project(&mut state, root).expect("a project opens");
        state.scene = Some(OpenScene {
            path: root.join("assets/scenes/main.rsn"),
            world: World::new(),
        });
        state
    }

    #[test]
    fn saving_a_selection_writes_a_prefab_and_rescans() {
        let scratch = Scratch::new("save-prefab");
        let root = scratch.path.clone();
        write_manifest(&root);
        let mut state = open(&root);

        let id = state.scene.as_mut().unwrap().world.spawn();
        state
            .scene
            .as_mut()
            .unwrap()
            .world
            .insert(id, Transform::default());
        state.selection = Selection::Entity(id as u64);

        let message = save_selection(&mut state, "crate").unwrap();
        assert!(message.contains("assets/prefabs/crate.prefab"), "{message}");
        assert!(root.join("assets/prefabs/crate.prefab").is_file());
        assert!(
            state
                .catalog
                .ending_with("prefab")
                .any(|entry| entry.path == "assets/prefabs/crate.prefab"),
            "the new prefab shows up in the catalog"
        );

        assert!(save_selection(&mut state, "a/b").is_err());
        assert!(save_selection(&mut state, "  ").is_err());
    }

    #[test]
    fn instantiating_places_the_prefab_as_one_undo_step() {
        let scratch = Scratch::new("instantiate-prefab");
        let root = scratch.path.clone();
        write_manifest(&root);
        let mut state = open(&root);

        // Build a two-entity tree and save it as a prefab.
        {
            let world = &mut state.scene.as_mut().unwrap().world;
            let parent = world.spawn();
            let child = world.spawn();
            world.insert(parent, Transform::from_position(Vec2::new(4.0, 4.0)));
            world.insert(child, Transform::from_position(Vec2::new(4.0, 8.0)));
            world.insert(child, Parent { entity: parent });
        }
        state.selection = Selection::Entity(0);
        save_selection(&mut state, "pair").unwrap();

        // A fresh scene, then place the prefab at the origin.
        state.scene.as_mut().unwrap().world = World::new();
        state.edits.clear();
        let depth = state.edits.depth();

        let message =
            instantiate(&mut state, "assets/prefabs/pair.prefab", Some(Vec2::ZERO)).unwrap();
        assert!(message.contains("instantiated"), "{message}");
        assert_eq!(state.edits.depth(), (depth.0 + 1, 0), "one undo step");
        assert!(state.dirty);

        let root_id = match state.selection {
            Selection::Entity(id) => id as EntityId,
            other => panic!("expected the placed root selected, got {other:?}"),
        };
        let world = &state.scene.as_ref().unwrap().world;
        assert_eq!(world.entities().len(), 2, "both entities placed");
        assert_eq!(
            world.get::<Transform>(root_id).unwrap().position,
            Vec2::ZERO,
            "the root lands on the drop point"
        );

        // One undo takes the whole fragment back out.
        crate::edit::undo(&mut state).unwrap();
        assert!(state.scene.as_ref().unwrap().world.entities().is_empty());
    }

    #[test]
    fn instantiating_without_a_scene_or_project_is_refused() {
        let mut state = EditorState::new();
        assert!(instantiate(&mut state, "assets/prefabs/x.prefab", None).is_err());
        assert!(save_selection(&mut state, "x").is_err());
    }
}

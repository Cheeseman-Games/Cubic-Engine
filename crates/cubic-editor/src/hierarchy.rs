//! The hierarchy panel's model: the entity tree as a flat list of rows.
//!
//! A world stores parent links, not a tree, so drawing the panel means
//! flattening the links into the rows a `Ui` can paint: each live entity once,
//! roots first, children indented under the parent that owns them, and
//! collapsed subtrees left out. The flattening is a pure function of the world
//! and the expansion state, so it is testable without a window.

use std::collections::{BTreeMap, HashSet};

use cubic_core::hierarchy;
use cubic_core::reflect::ComponentRegistry;
use cubic_core::world::{EntityId, World};

/// Which subtrees the panel has collapsed. Everything starts expanded, so the
/// set only holds what the user closed.
#[derive(Default)]
pub struct HierarchyState {
    collapsed: HashSet<EntityId>,
}

impl HierarchyState {
    /// Whether `id`'s children are shown.
    pub fn is_expanded(&self, id: EntityId) -> bool {
        !self.collapsed.contains(&id)
    }

    /// Flips whether `id`'s children are shown.
    pub fn toggle(&mut self, id: EntityId) {
        if !self.collapsed.remove(&id) {
            self.collapsed.insert(id);
        }
    }

    /// Shows `id`'s children.
    pub fn expand(&mut self, id: EntityId) {
        self.collapsed.remove(&id);
    }

    /// Hides `id`'s children.
    pub fn collapse(&mut self, id: EntityId) {
        self.collapsed.insert(id);
    }

    /// Forgets every collapse, for when the scene underneath is replaced.
    pub fn clear(&mut self) {
        self.collapsed.clear();
    }
}

/// One line of the hierarchy: an entity, how deep it sits, and what its row
/// can offer.
pub struct Row {
    /// The entity this row stands for.
    pub id: EntityId,
    /// How many levels under a root it sits, for indentation.
    pub depth: usize,
    /// Its effective parent, if it has one.
    pub parent: Option<EntityId>,
    /// How many direct children it has, whether or not they are shown.
    pub children: usize,
    /// The registered components it holds, in name order — what a "remove
    /// component" menu offers.
    pub components: Vec<&'static str>,
    /// Registered addable components it does not hold — what an "add
    /// component" menu offers.
    pub addable: Vec<&'static str>,
}

/// Flattens `world` into the rows the panel draws.
///
/// The child index is built from [`hierarchy::parent_map`], not the raw links,
/// so a hand-edited cycle in the scene cannot make the walk loop or show an
/// entity twice — the map already dropped the one link a tree cannot show.
pub fn layout(world: &World, components: &ComponentRegistry, state: &HierarchyState) -> Vec<Row> {
    let parents = hierarchy::parent_map(world);
    let mut children: BTreeMap<EntityId, Vec<EntityId>> = BTreeMap::new();
    for (child, parent) in &parents {
        children.entry(*parent).or_default().push(*child);
    }

    let mut rows = Vec::new();
    let mut pending: Vec<(EntityId, usize)> = hierarchy::roots(world)
        .into_iter()
        .rev()
        .map(|id| (id, 0))
        .collect();
    while let Some((id, depth)) = pending.pop() {
        let held: Vec<&'static str> = components
            .on_entity(world, id)
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        let addable: Vec<&'static str> = components
            .entries()
            .filter(|entry| entry.is_addable() && !held.contains(&entry.name))
            .map(|entry| entry.name)
            .collect();
        let child_ids = children.get(&id);
        rows.push(Row {
            id,
            depth,
            parent: parents.get(&id).copied(),
            children: child_ids.map_or(0, Vec::len),
            components: held,
            addable,
        });
        if let Some(child_ids) = child_ids
            && state.is_expanded(id)
        {
            for child in child_ids.iter().rev() {
                pending.push((*child, depth + 1));
            }
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use cubic_core::components::Transform;
    use cubic_core::hierarchy::Parent;

    fn world_with_links() -> World {
        let mut world = World::new();
        for _ in 0..4 {
            let id = world.spawn();
            world.insert(id, Transform::default());
        }
        world.insert(1, Parent { entity: 0 });
        world.insert(3, Parent { entity: 0 });
        world.insert(2, Parent { entity: 1 });
        world
    }

    fn ids(rows: &[Row]) -> Vec<EntityId> {
        rows.iter().map(|row| row.id).collect()
    }

    #[test]
    fn rows_come_out_roots_first_then_indented_children() {
        let world = world_with_links();
        let rows = layout(
            &world,
            &ComponentRegistry::engine_defaults(),
            &HierarchyState::default(),
        );
        assert_eq!(ids(&rows), vec![0, 1, 2, 3]);
        assert_eq!(
            rows.iter().map(|row| row.depth).collect::<Vec<_>>(),
            vec![0, 1, 2, 1]
        );
        assert_eq!(rows[0].children, 2, "entity 0 has two direct children");
        assert_eq!(rows[2].parent, Some(1));
    }

    #[test]
    fn a_collapsed_node_hides_its_subtree() {
        let world = world_with_links();
        let mut state = HierarchyState::default();
        state.collapse(0);
        let rows = layout(&world, &ComponentRegistry::engine_defaults(), &state);
        assert_eq!(ids(&rows), vec![0], "both of 0's children are gone");
        assert_eq!(rows[0].children, 2, "the row still knows it has children");
    }

    #[test]
    fn rows_list_what_can_be_added_and_removed() {
        let world = world_with_links();
        let rows = layout(
            &world,
            &ComponentRegistry::engine_defaults(),
            &HierarchyState::default(),
        );
        assert_eq!(
            rows[0].components,
            vec!["Transform"],
            "a root holds no Parent"
        );
        assert!(
            rows[0].addable.is_empty(),
            "Transform is already held and Parent is not addable"
        );
        assert_eq!(rows[1].components, vec!["Parent", "Transform"]);
    }

    #[test]
    fn toggle_round_trips_expansion() {
        let mut state = HierarchyState::default();
        assert!(state.is_expanded(5));
        state.toggle(5);
        assert!(!state.is_expanded(5));
        state.toggle(5);
        assert!(state.is_expanded(5));
    }
}

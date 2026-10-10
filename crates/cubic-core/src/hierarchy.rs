//! The scene hierarchy: which entity is whose parent.
//!
//! A [`Parent`] component makes one entity the child of another, and a child's
//! transform composes inside its parent's (see
//! [`Transform::to_mat4_in`](crate::components::Transform::to_mat4_in)). This
//! module answers what the editor's hierarchy panel asks of a world: who is
//! whose parent, who are the roots, whether one entity stands above another,
//! and what a delete would take with it.
//!
//! # Stale links
//!
//! Links are read from the components as stored, so a hand-edited `.rsn` file
//! may name a parent that no longer exists, an entity may name itself, or two
//! entities may name each other. None of that may hang the panel that draws
//! the tree, so [`parent_map`] first drops every link a tree cannot show —
//! dead parents, self-parents, and one link per cycle — and [`roots`] and the
//! panel are built on what survives. The raw queries ([`parent_of`],
//! [`children_of`], [`is_ancestor`], [`descendants`]) never loop forever
//! either: a link already walked ends the search.

use std::collections::{BTreeMap, HashSet};

use crate::world::{EntityId, World};

/// Makes one entity the child of another.
///
/// Children inherit their parent's transform, so the pair reads as one object
/// in the viewport and one row-group in the hierarchy panel. An entity with no
/// `Parent` is a root.
#[cfg_attr(
    feature = "reflect",
    derive(crate::reflect::Component, crate::reflect::Inspectable)
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Parent {
    /// The parent's id.
    ///
    /// A scene file may name an entity that is gone, or — if edited by hand —
    /// this very one. Every reader here treats such a link as no parent at
    /// all rather than failing, so a stale link degrades into a root instead
    /// of wedging the tree.
    #[cfg_attr(feature = "reflect", inspect(skip))]
    pub entity: EntityId,
}

/// The parent `id` names, exactly as stored.
///
/// [`parent_map`] is the one to build a tree from; this is the raw link, dead
/// parent and all.
pub fn parent_of(world: &World, id: EntityId) -> Option<EntityId> {
    world.get::<Parent>(id).map(|parent| parent.entity)
}

/// Every entity that names `id` as its parent, ascending.
pub fn children_of(world: &World, id: EntityId) -> Vec<EntityId> {
    world
        .iter::<Parent>()
        .filter(|(_, parent)| parent.entity == id)
        .map(|(child, _)| child)
        .collect()
}

/// The links a tree can show: child → parent, minus the ones it cannot.
///
/// Dropped from the stored links: a parent that is not alive, an entity that
/// names itself, and one link per cycle. Each cycle is walked once from its
/// lowest id — following parents until the next one is already on the walk —
/// and the link that would close that loop is dropped, which leaves exactly
/// one root inside the cycle: the lowest member's child. Walking in id order
/// is what makes *which* link goes a fact of the world rather than of
/// iteration order.
///
/// What comes back is therefore a forest — no loops, no dangling edges — and
/// every live entity with a stored parent either appears here or under
/// something that does.
pub fn parent_map(world: &World) -> BTreeMap<EntityId, EntityId> {
    let alive: HashSet<EntityId> = world.entities().into_iter().collect();
    let mut links: BTreeMap<EntityId, EntityId> = BTreeMap::new();
    for (child, parent) in world.iter::<Parent>() {
        if parent.entity != child && alive.contains(&parent.entity) {
            links.insert(child, parent.entity);
        }
    }

    // Walk each unvisited link upward until it reaches a root or a link whose
    // fate is already decided, then settle the whole path at once. Walking in
    // id order is what makes *which* link a cycle loses a fact of the world
    // rather than of iteration order.
    let mut settled: BTreeMap<EntityId, Option<EntityId>> = BTreeMap::new();
    for start in links.keys().copied().collect::<Vec<_>>() {
        if settled.contains_key(&start) {
            continue;
        }
        let mut path = Vec::new();
        let mut walked = HashSet::new();
        let mut node = start;
        let tail = loop {
            path.push(node);
            walked.insert(node);
            match links.get(&node) {
                None => break None,
                Some(&next) if settled.contains_key(&next) => break Some(next),
                Some(&next) if walked.contains(&next) => break None,
                Some(&next) => node = next,
            }
        };
        for pair in path.windows(2) {
            settled.insert(pair[0], Some(pair[1]));
        }
        settled.insert(*path.last().expect("the path has at least one node"), tail);
    }

    settled
        .into_iter()
        .filter_map(|(child, parent)| parent.map(|parent| (child, parent)))
        .collect()
}

/// The entities with no effective parent, ascending — where a tree starts.
///
/// An entity whose parent is dead, itself, or part of a cycle it lost is a
/// root too: better drawn loose than not drawn at all.
pub fn roots(world: &World) -> Vec<EntityId> {
    let parents = parent_map(world);
    world
        .entities()
        .into_iter()
        .filter(|id| !parents.contains_key(id))
        .collect()
}

/// Whether `ancestor` stands above `of` — its parent, or its parent's parent,
/// and so on up.
///
/// The entity itself never counts as its own ancestor. A cycle ends the search
/// instead of chasing itself: what is found before then is reported, what is
/// not is `false`.
pub fn is_ancestor(world: &World, ancestor: EntityId, of: EntityId) -> bool {
    let mut walked = HashSet::new();
    let mut node = of;
    while let Some(parent) = parent_of(world, node) {
        if parent == ancestor {
            return true;
        }
        if !walked.insert(node) {
            return false;
        }
        node = parent;
    }
    false
}

/// Everything under `id`, one level at a time, ascending and without repeats.
///
/// `id` itself is not included. This is the set a delete takes with it, and
/// the traversal never revisits an entity even if the links form a cycle.
pub fn descendants(world: &World, id: EntityId) -> Vec<EntityId> {
    let mut found = Vec::new();
    let mut walked = HashSet::from([id]);
    let mut pending = vec![id];
    while let Some(node) = pending.pop() {
        for child in children_of(world, node) {
            if walked.insert(child) {
                found.push(child);
                pending.push(child);
            }
        }
    }
    found.sort_unstable();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A world of `count` entities, each with a transform so they are live.
    fn world_of(count: u32) -> World {
        let mut world = World::new();
        for _ in 0..count {
            let id = world.spawn();
            world.insert(id, crate::components::Transform::default());
        }
        world
    }

    fn link(world: &mut World, child: EntityId, parent: EntityId) {
        world.insert(child, Parent { entity: parent });
    }

    #[test]
    fn a_parent_makes_a_child_and_a_link_back() {
        let mut world = world_of(2);
        link(&mut world, 1, 0);

        assert_eq!(parent_of(&world, 1), Some(0));
        assert_eq!(parent_of(&world, 0), None, "roots hold no link");
        assert_eq!(children_of(&world, 0), vec![1]);
        assert_eq!(children_of(&world, 1), Vec::<EntityId>::new());
    }

    #[test]
    fn children_come_back_ascending() {
        let mut world = world_of(4);
        link(&mut world, 3, 0);
        link(&mut world, 1, 0);
        link(&mut world, 2, 0);

        assert_eq!(children_of(&world, 0), vec![1, 2, 3]);
    }

    #[test]
    fn a_dead_parent_reads_as_no_parent() {
        let mut world = world_of(2);
        link(&mut world, 1, 0);
        world.despawn(0);

        assert_eq!(parent_of(&world, 1), Some(0), "the raw link is still there");
        assert_eq!(
            parent_map(&world),
            BTreeMap::new(),
            "but no tree can show it"
        );
        assert_eq!(roots(&world), vec![1], "so the child is a root");
    }

    #[test]
    fn an_entity_that_names_itself_is_a_root() {
        let mut world = world_of(1);
        link(&mut world, 0, 0);

        assert_eq!(parent_map(&world), BTreeMap::new());
        assert_eq!(roots(&world), vec![0]);
        assert_eq!(descendants(&world, 0), Vec::<EntityId>::new());
    }

    #[test]
    fn a_cycle_is_broken_once_and_deterministically() {
        let mut world = world_of(3);
        link(&mut world, 0, 2);
        link(&mut world, 2, 1);
        link(&mut world, 1, 0);

        // The walk starts at 0 and follows 0 → 2 → 1; the link closing that
        // walk (1 → 0) is the one dropped, leaving 1 as the cycle's root.
        assert_eq!(
            parent_map(&world).into_iter().collect::<Vec<_>>(),
            vec![(0, 2), (2, 1)]
        );
        assert_eq!(roots(&world), vec![1]);
        assert!(is_ancestor(&world, 0, 1), "raw links still form the loop");
    }

    #[test]
    fn ancestors_reach_the_root_and_stop_at_cycles() {
        let mut world = world_of(3);
        link(&mut world, 2, 1);
        link(&mut world, 1, 0);

        assert!(is_ancestor(&world, 0, 2));
        assert!(is_ancestor(&world, 1, 2));
        assert!(!is_ancestor(&world, 2, 0), "an ancestor is strictly above");
        assert!(!is_ancestor(&world, 0, 0));
        assert!(!is_ancestor(&world, 9, 2), "an entity nobody links to");

        // Now close the loop: 0 → 2 → 1 → 0.
        link(&mut world, 0, 2);
        assert!(
            is_ancestor(&world, 0, 0),
            "in a cycle everything reaches everything"
        );
        assert!(!is_ancestor(&world, 9, 0), "and an outsider still does not");
    }

    #[test]
    fn descendants_are_the_whole_subtree() {
        let mut world = world_of(5);
        link(&mut world, 1, 0);
        link(&mut world, 2, 0);
        link(&mut world, 3, 1);
        link(&mut world, 4, 9);

        assert_eq!(descendants(&world, 0), vec![1, 2, 3]);
        assert_eq!(descendants(&world, 1), vec![3]);
        assert_eq!(descendants(&world, 3), Vec::<EntityId>::new());
        assert_eq!(descendants(&world, 4), Vec::<EntityId>::new());
    }

    #[test]
    fn descendants_of_a_cycle_terminate() {
        let mut world = world_of(3);
        link(&mut world, 1, 0);
        link(&mut world, 2, 1);
        link(&mut world, 0, 2);

        assert_eq!(descendants(&world, 0), vec![1, 2]);
    }

    #[test]
    fn a_tree_draws_roots_then_their_children() {
        let mut world = world_of(5);
        link(&mut world, 1, 0);
        link(&mut world, 3, 2);

        assert_eq!(roots(&world), vec![0, 2, 4]);
        assert_eq!(
            parent_map(&world).into_iter().collect::<Vec<_>>(),
            vec![(1, 0), (3, 2)]
        );
    }
}

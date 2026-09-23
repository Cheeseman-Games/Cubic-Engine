use std::any::{Any, TypeId};
use std::collections::HashMap;

/// Entity ids are plain indices into sparse component stores. Ids are
/// recycled on despawn, so a stale id may alias a future entity — systems
/// must not cache ids across ticks.
pub type EntityId = u32;

/// Backing store for exactly one component type: a sparse `Vec<Option<T>>`
/// indexed by `EntityId`. Nothing leaks out of `World` except downcast
/// handles, so systems access dense contiguous memory with no per-entity
/// hashing or indirection.
trait ComponentStore {
    fn clear_at(&mut self, id: EntityId);
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<T: Any + 'static> ComponentStore for Vec<Option<T>> {
    fn clear_at(&mut self, id: EntityId) {
        if let Some(slot) = self.get_mut(id as usize) {
            *slot = None;
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// The entity / component store at the heart of the engine.
///
/// Each component type `T` lives in its own dense `Vec<Option<T>>`,
/// guaranteeing O(1) access and cache-friendly iteration. Systems that want
/// the full speed take a `&mut` handle to a whole store (`store_mut::<T>`)
/// instead of calling `get`/`insert` repeatedly.
#[derive(Default)]
pub struct World {
    next_id: EntityId,
    free: Vec<EntityId>,
    stores: HashMap<TypeId, Box<dyn ComponentStore>>,
}

impl World {
    pub fn new() -> Self {
        Self::default()
    }

    /// Immutable handle to the whole dense store for `T`.
    pub fn store<T: Any + 'static>(&self) -> Option<&Vec<Option<T>>> {
        let store = self.stores.get(&TypeId::of::<T>())?;
        store.as_any().downcast_ref::<Vec<Option<T>>>()
    }

    /// Mutable handle to the whole dense store for `T`, created on demand so
    /// systems can freely pair with other stores in separate borrow scopes.
    pub fn store_mut<T: Any + 'static>(&mut self) -> &mut Vec<Option<T>> {
        let tid = TypeId::of::<T>();
        let store = self
            .stores
            .entry(tid)
            .or_insert_with(|| Box::new(Vec::<Option<T>>::new()));
        store
            .as_any_mut()
            .downcast_mut::<Vec<Option<T>>>()
            .expect("component storage registered under a different type")
    }

    pub fn spawn(&mut self) -> EntityId {
        self.free.pop().unwrap_or_else(|| {
            let id = self.next_id;
            self.next_id += 1;
            id
        })
    }

    pub fn despawn(&mut self, id: EntityId) {
        for store in self.stores.values_mut() {
            store.clear_at(id);
        }
        self.free.push(id);
    }

    pub fn get<T: Any + 'static>(&self, id: EntityId) -> Option<&T> {
        self.store::<T>()?.get(id as usize)?.as_ref()
    }

    pub fn get_mut<T: Any + 'static>(&mut self, id: EntityId) -> Option<&mut T> {
        self.store_mut::<T>().get_mut(id as usize)?.as_mut()
    }

    pub fn insert<T: Any + 'static>(&mut self, id: EntityId, component: T) {
        let store = self.store_mut::<T>();
        if store.len() <= id as usize {
            store.resize_with(id as usize + 1, || None);
        }
        store[id as usize] = Some(component);
    }

    pub fn remove<T: Any + 'static>(&mut self, id: EntityId) -> Option<T> {
        self.store_mut::<T>().get_mut(id as usize)?.take()
    }

    pub fn has<T: Any + 'static>(&self, id: EntityId) -> bool {
        self.get::<T>(id).is_some()
    }

    pub fn ids_with<T: Any + 'static>(&self) -> Vec<EntityId> {
        self.iter::<T>().map(|(id, _)| id).collect()
    }

    pub fn iter<T: Any + 'static>(&self) -> impl Iterator<Item = (EntityId, &T)> {
        let store = self.store::<T>();
        store.into_iter().flat_map(|slots| {
            slots
                .iter()
                .enumerate()
                .filter_map(|(id, slot)| slot.as_ref().map(|c| (id as EntityId, c)))
        })
    }

    pub fn iter_mut<T: Any + 'static>(&mut self) -> impl Iterator<Item = (EntityId, &mut T)> {
        let store = self.store_mut::<T>();
        store
            .iter_mut()
            .enumerate()
            .filter_map(|(id, slot)| slot.as_mut().map(|c| (id as EntityId, c)))
    }
}
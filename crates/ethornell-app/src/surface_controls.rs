use ethornell_vm::GraphInputDescriptor;
use std::collections::{BTreeMap, BTreeSet};

/// Identity of one target private-control set.
///
/// A CDspObjWindow may be referenced by more than one DCIPIcon/DCIPIconEx
/// processor at the same time. Target processors own their child/Virtual item
/// arrays independently; the window itself does not own one global item set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum SurfaceControlOwner {
    Surface(i32),
    InputObject(i32),
}

#[derive(Debug, Clone)]
struct SurfaceControlSet {
    surface: i32,
    descriptor: GraphInputDescriptor,
    layers: BTreeSet<i32>,
}

#[derive(Debug, Default)]
pub(crate) struct SurfaceControlRegistry {
    sets: BTreeMap<SurfaceControlOwner, SurfaceControlSet>,
}

impl SurfaceControlRegistry {
    pub(crate) fn is_unchanged(
        &self,
        owner: SurfaceControlOwner,
        surface: i32,
        descriptor: &GraphInputDescriptor,
    ) -> bool {
        self.sets
            .get(&owner)
            .is_some_and(|set| set.surface == surface && set.descriptor == *descriptor)
    }

    /// Number of materialized native-control layers attached to `surface`
    /// across all independent owners/processors.
    pub(crate) fn layer_count(&self, surface: i32) -> usize {
        self.sets
            .values()
            .filter(|set| set.surface == surface)
            .map(|set| set.layers.len())
            .sum()
    }

    pub(crate) fn owner_layer_count(&self, owner: SurfaceControlOwner) -> usize {
        self.sets.get(&owner).map_or(0, |set| set.layers.len())
    }

    pub(crate) fn replace(
        &mut self,
        owner: SurfaceControlOwner,
        surface: i32,
        descriptor: GraphInputDescriptor,
        layers: BTreeSet<i32>,
    ) -> BTreeSet<i32> {
        self.sets
            .insert(
                owner,
                SurfaceControlSet {
                    surface,
                    descriptor,
                    layers,
                },
            )
            .map(|set| set.layers)
            .unwrap_or_default()
    }

    /// Remove exactly one target control owner. Releasing one DCIPIcon must
    /// not destroy another processor's children merely because both reference
    /// the same CDspObjWindow.
    pub(crate) fn remove_owner(&mut self, owner: SurfaceControlOwner) -> BTreeSet<i32> {
        self.sets
            .remove(&owner)
            .map(|set| set.layers)
            .unwrap_or_default()
    }

    /// Remove every owner attached to a destroyed Window/surface.
    pub(crate) fn remove_surface(&mut self, surface: i32) -> BTreeSet<i32> {
        let owners = self
            .sets
            .iter()
            .filter_map(|(&owner, set)| (set.surface == surface).then_some(owner))
            .collect::<Vec<_>>();
        let mut removed = BTreeSet::new();
        for owner in owners {
            if let Some(set) = self.sets.remove(&owner) {
                removed.extend(set.layers);
            }
        }
        removed
    }

    pub(crate) fn contains_layer(&self, layer: i32) -> bool {
        self.sets.values().any(|set| set.layers.contains(&layer))
    }

    pub(crate) fn contains_layer_for_owner(&self, owner: SurfaceControlOwner, layer: i32) -> bool {
        self.sets
            .get(&owner)
            .is_some_and(|set| set.layers.contains(&layer))
    }

    pub(crate) fn update_descriptor(
        &mut self,
        owner: SurfaceControlOwner,
        descriptor: GraphInputDescriptor,
    ) {
        if let Some(set) = self.sets.get_mut(&owner) {
            set.descriptor = descriptor;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_returns_only_the_same_owners_stale_layers() {
        let mut registry = SurfaceControlRegistry::default();
        let descriptor = GraphInputDescriptor::default();
        let owner_a = SurfaceControlOwner::InputObject(100);
        let owner_b = SurfaceControlOwner::InputObject(101);

        assert!(
            registry
                .replace(owner_a, 7, descriptor.clone(), BTreeSet::from([1, 2]))
                .is_empty()
        );
        assert!(registry.is_unchanged(owner_a, 7, &descriptor));

        assert!(
            registry
                .replace(owner_b, 7, descriptor.clone(), BTreeSet::from([8]))
                .is_empty()
        );
        assert_eq!(registry.layer_count(7), 3);
        assert_eq!(registry.owner_layer_count(owner_a), 2);
        assert_eq!(registry.owner_layer_count(owner_b), 1);

        assert_eq!(
            registry.replace(owner_a, 7, descriptor, BTreeSet::from([3])),
            BTreeSet::from([1, 2])
        );
        assert!(registry.contains_layer(3));
        assert!(registry.contains_layer_for_owner(owner_a, 3));
        assert!(!registry.contains_layer_for_owner(owner_b, 3));
        assert!(registry.contains_layer_for_owner(owner_b, 8));
        assert_eq!(registry.layer_count(7), 2);

        assert_eq!(registry.remove_owner(owner_a), BTreeSet::from([3]));
        assert!(registry.contains_layer(8));
        assert_eq!(registry.layer_count(7), 1);
        assert_eq!(registry.remove_surface(7), BTreeSet::from([8]));
        assert_eq!(registry.layer_count(7), 0);
    }
}

use ethornell_vm::GraphInputDescriptor;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Default)]
pub(crate) struct SurfaceControlRegistry {
    layers: BTreeMap<i32, BTreeSet<i32>>,
    descriptors: BTreeMap<i32, GraphInputDescriptor>,
}

impl SurfaceControlRegistry {
    pub(crate) fn is_unchanged(&self, surface: i32, descriptor: &GraphInputDescriptor) -> bool {
        self.descriptors.get(&surface) == Some(descriptor)
    }

    pub(crate) fn layer_count(&self, surface: i32) -> usize {
        self.layers.get(&surface).map_or(0, BTreeSet::len)
    }

    pub(crate) fn replace(
        &mut self,
        surface: i32,
        descriptor: GraphInputDescriptor,
        layers: BTreeSet<i32>,
    ) -> BTreeSet<i32> {
        self.descriptors.insert(surface, descriptor);
        self.layers.insert(surface, layers).unwrap_or_default()
    }

    pub(crate) fn remove(&mut self, surface: i32) -> BTreeSet<i32> {
        self.descriptors.remove(&surface);
        self.layers.remove(&surface).unwrap_or_default()
    }

    pub(crate) fn contains_layer(&self, layer: i32) -> bool {
        self.layers.values().any(|layers| layers.contains(&layer))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_returns_stale_layers() {
        let mut registry = SurfaceControlRegistry::default();
        let descriptor = GraphInputDescriptor::default();
        assert!(registry
            .replace(7, descriptor.clone(), BTreeSet::from([1, 2]))
            .is_empty());
        assert!(registry.is_unchanged(7, &descriptor));
        assert_eq!(
            registry.replace(7, descriptor, BTreeSet::from([3])),
            BTreeSet::from([1, 2])
        );
        assert!(registry.contains_layer(3));
        assert_eq!(registry.remove(7), BTreeSet::from([3]));
    }
}

use std::collections::BTreeMap;

#[derive(Debug, Default)]
pub(crate) struct GraphLinkRegistry {
    parents: BTreeMap<i32, i32>,
}

impl GraphLinkRegistry {
    pub(crate) fn parent(&self, object: i32) -> Option<i32> {
        self.parents.get(&object).copied()
    }

    pub(crate) fn relations(&self) -> Vec<(i32, i32)> {
        self.parents
            .iter()
            .map(|(&child, &parent)| (child, parent))
            .collect()
    }

    pub(crate) fn unlink(&mut self, object: i32) {
        self.parents.remove(&object);
        self.parents.retain(|_, parent| *parent != object);
    }

    pub(crate) fn set_parent(
        &mut self,
        object: i32,
        parent: i32,
        object_exists: bool,
        parent_exists: bool,
    ) -> i32 {
        if !object_exists {
            return 255;
        }
        if parent == object || (parent != 0 && !parent_exists) {
            return 11;
        }
        if parent == 0 {
            self.parents.remove(&object);
            return 0;
        }

        let mut ancestor = parent;
        while let Some(next) = self.parents.get(&ancestor).copied() {
            if next == object {
                return 13;
            }
            ancestor = next;
        }
        self.parents.insert(object, parent);
        0
    }
}

#[cfg(test)]
mod tests {
    use super::GraphLinkRegistry;

    #[test]
    fn rejects_missing_handles_self_links_and_cycles() {
        let mut links = GraphLinkRegistry::default();

        assert_eq!(links.set_parent(1, 2, false, true), 255);
        assert_eq!(links.set_parent(1, 2, true, false), 11);
        assert_eq!(links.set_parent(1, 1, true, true), 11);
        assert_eq!(links.set_parent(1, 2, true, true), 0);
        assert_eq!(links.parent(1), Some(2));
        assert_eq!(links.set_parent(2, 1, true, true), 13);
        assert_eq!(links.set_parent(1, 0, true, true), 0);
        assert_eq!(links.parent(1), None);
    }
}

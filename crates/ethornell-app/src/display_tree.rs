use std::collections::{BTreeMap, BTreeSet};

/// Native BGI display-object classes encoded in the high byte of a display
/// handle. The current runtime still has a few legacy sequential handles, so
/// `Generic` remains a first-class kind rather than coercing them to sprites.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum NativeDisplayKind {
    Root,
    BackN,
    BackB,
    BackS,
    BackF,
    BackD,
    BackDst,
    BackGrd,
    BackRpl,
    BackStr,
    BackRtt,
    BackMsc,
    BackMl,
    Sprite,
    Filter,
    Effector,
    Map,
    Landscape,
    Window,
    ParticleScreen,
    RainScreen,
    Knob,
    Group,
    Generic,
}

impl NativeDisplayKind {
    pub(crate) fn from_handle(handle: i32) -> Self {
        if handle == 0 {
            return Self::Root;
        }
        match (handle as u32) & 0xff00_0000 {
            0x8000_0000 => Self::Sprite,
            0x9000_0000 => Self::Filter,
            0x9100_0000 => Self::Effector,
            0xa000_0000 => Self::Map,
            0xa100_0000 => Self::Landscape,
            0xb000_0000 => Self::Window,
            0xc000_0000 => Self::ParticleScreen,
            0xc100_0000 => Self::RainScreen,
            0xf000_0000 => Self::Knob,
            0xf100_0000 => Self::Group,
            _ => Self::Generic,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct NativeDisplayObjectState {
    /// This structure owns portable hierarchy/chain bookkeeping only.
    /// The single target `CDspObjLayout32` image is stored in
    /// `RuntimeGraphObjectProperties`; keeping a second copy here would make
    /// reverse-engineered fields diverge silently.
    pub(crate) kind: NativeDisplayKind,
    pub(crate) parent: Option<i32>,
    pub(crate) children: Vec<i32>,
    pub(crate) enabled: bool,
    pub(crate) hidden: bool,
    pub(crate) active: bool,
    pub(crate) local_x: f32,
    pub(crate) local_y: f32,
    pub(crate) local_z: i32,
    pub(crate) transform_x: f32,
    pub(crate) transform_y: f32,
    pub(crate) transform_z: i32,
    /// Script-visible priority/depth used by the portable renderer.
    pub(crate) chain_depth: i32,
    /// Key cached in the target CObjectManager intrusive-chain node. The
    /// target obtains it from CDspObj vtable+0x1C (sub_41B0C0) when inserting
    /// or reinserting the object; it is intentionally separate from raw
    /// priority because transformed fixed Z occupies the low key bits.
    pub(crate) manager_sort_key: Option<u32>,
    pub(crate) creation_serial: u64,
}

impl NativeDisplayObjectState {
    fn new(kind: NativeDisplayKind, creation_serial: u64) -> Self {
        Self {
            kind,
            parent: None,
            children: Vec::new(),
            enabled: true,
            hidden: false,
            active: true,
            local_x: 0.0,
            local_y: 0.0,
            local_z: 0,
            transform_x: 0.0,
            transform_y: 0.0,
            transform_z: 0,
            chain_depth: 0,
            manager_sort_key: None,
            creation_serial,
        }
    }

    fn drawable(&self) -> bool {
        self.enabled && !self.hidden && self.active
    }
}

/// Portable mirror of the target's independent depth-sorted intrusive display
/// chain.  Parent/child relations are intentionally not stored here: target
/// transforms and final composition order are separate structures.
#[derive(Debug, Default)]
struct NativeDisplayChain {
    handles: Vec<i32>,
}

impl NativeDisplayChain {
    fn insert_after_equal_depth(
        &mut self,
        objects: &BTreeMap<i32, NativeDisplayObjectState>,
        handle: i32,
        depth: i32,
    ) {
        self.handles.retain(|candidate| *candidate != handle);
        let insertion = self
            .handles
            .iter()
            .position(|candidate| {
                objects
                    .get(candidate)
                    .is_some_and(|object| object.chain_depth > depth)
            })
            .unwrap_or(self.handles.len());
        self.handles.insert(insertion, handle);
    }

    fn insert_after_equal_native_sort_key(
        &mut self,
        objects: &BTreeMap<i32, NativeDisplayObjectState>,
        handle: i32,
        depth: i32,
        sort_key: u32,
    ) {
        self.handles.retain(|candidate| *candidate != handle);
        let insertion = self
            .handles
            .iter()
            .position(|candidate| {
                let Some(candidate) = objects.get(candidate) else {
                    return false;
                };
                candidate.chain_depth > depth
                    || (candidate.chain_depth == depth
                        && candidate
                            .manager_sort_key
                            .is_some_and(|candidate_key| candidate_key > sort_key))
            })
            .unwrap_or(self.handles.len());
        self.handles.insert(insertion, handle);
    }

    fn remove(&mut self, handle: i32) {
        self.handles.retain(|candidate| *candidate != handle);
    }

    fn position(&self, handle: i32) -> Option<u64> {
        self.handles
            .iter()
            .position(|candidate| *candidate == handle)
            .map(|position| position as u64)
    }

    fn handles(&self) -> &[i32] {
        &self.handles
    }
}

/// Portable ownership model for the two structural relations recovered from
/// CObjectManager/CDspObj: parent/child transform propagation and the independent
/// depth-sorted display chain. Exact target `CDspObj` fields are deliberately
/// kept in one place (`RuntimeGraphObjectProperties`) rather than duplicated here.
#[derive(Debug)]
pub(crate) struct NativeDisplayTree {
    objects: BTreeMap<i32, NativeDisplayObjectState>,
    /// Target `CObjectManager` keeps a depth-sorted intrusive chain in
    /// addition to the parent/child hierarchy.  This vector mirrors that
    /// ordering: ascending depth, equal-depth insertion after existing nodes.
    display_chain: NativeDisplayChain,
    next_creation_serial: u64,
}

impl Default for NativeDisplayTree {
    fn default() -> Self {
        let mut tree = Self {
            objects: BTreeMap::new(),
            display_chain: NativeDisplayChain::default(),
            next_creation_serial: 1,
        };
        tree.objects
            .insert(0, NativeDisplayObjectState::new(NativeDisplayKind::Root, 0));
        tree.display_chain.handles.push(0);
        tree
    }
}

impl NativeDisplayTree {
    pub(crate) fn contains(&self, handle: i32) -> bool {
        self.objects.contains_key(&handle)
    }

    pub(crate) fn register(&mut self, handle: i32, kind: NativeDisplayKind) -> u64 {
        if handle == -1 {
            return 0;
        }
        if let Some(object) = self.objects.get_mut(&handle) {
            if object.kind == NativeDisplayKind::Generic && kind != NativeDisplayKind::Generic {
                object.kind = kind;
            }
            return object.creation_serial;
        }
        let serial = self.next_creation_serial;
        self.next_creation_serial = self.next_creation_serial.saturating_add(1);
        self.objects
            .insert(handle, NativeDisplayObjectState::new(kind, serial));
        self.insert_chain_after_equal_depth(handle, 0);
        serial
    }

    /// Replaces the runtime type of an existing handle. The target background
    /// manager destroys and reconstructs its singleton object when the class
    /// selector changes, so ordinary inferred registration is insufficient.
    pub(crate) fn replace_kind(&mut self, handle: i32, kind: NativeDisplayKind) {
        self.register(handle, kind);
        if let Some(object) = self.objects.get_mut(&handle) {
            object.kind = kind;
        }
    }

    pub(crate) fn register_inferred(&mut self, handle: i32) -> u64 {
        self.register(handle, NativeDisplayKind::from_handle(handle))
    }

    pub(crate) fn remove(&mut self, handle: i32) -> bool {
        if handle == 0 {
            return false;
        }
        let Some(object) = self.objects.remove(&handle) else {
            return false;
        };
        self.display_chain.remove(handle);
        if let Some(parent) = object.parent {
            if let Some(parent) = self.objects.get_mut(&parent) {
                parent.children.retain(|child| *child != handle);
            }
        }
        for child in object.children {
            if let Some(child) = self.objects.get_mut(&child) {
                child.parent = None;
            }
        }
        true
    }

    pub(crate) fn set_parent(&mut self, child: i32, parent: i32) -> Result<(), i32> {
        if child == 0 || child == parent || !self.contains(child) {
            return Err(11);
        }
        if parent != 0 && !self.contains(parent) {
            return Err(11);
        }

        let mut ancestor = Some(parent);
        let mut depth = 0usize;
        while let Some(handle) = ancestor {
            if handle == child {
                return Err(13);
            }
            ancestor = self.objects.get(&handle).and_then(|object| object.parent);
            depth += 1;
            if depth > self.objects.len() {
                return Err(13);
            }
        }

        let old_parent = self.objects.get(&child).and_then(|object| object.parent);
        if let Some(old_parent) = old_parent {
            if let Some(parent) = self.objects.get_mut(&old_parent) {
                parent.children.retain(|candidate| *candidate != child);
            }
        }
        if let Some(object) = self.objects.get_mut(&child) {
            object.parent = (parent != 0).then_some(parent);
        }
        if parent != 0 {
            if let Some(parent) = self.objects.get_mut(&parent) {
                if !parent.children.contains(&child) {
                    parent.children.push(child);
                }
            }
        }
        Ok(())
    }

    pub(crate) fn kind(&self, handle: i32) -> Option<NativeDisplayKind> {
        self.objects.get(&handle).map(|object| object.kind)
    }

    pub(crate) fn parent(&self, handle: i32) -> Option<i32> {
        self.objects.get(&handle).and_then(|object| object.parent)
    }

    pub(crate) fn children(&self, handle: i32) -> Vec<i32> {
        self.objects
            .get(&handle)
            .map(|object| object.children.clone())
            .unwrap_or_default()
    }

    pub(crate) fn local_position(&self, handle: i32) -> Option<(f32, f32)> {
        self.objects
            .get(&handle)
            .map(|object| (object.local_x, object.local_y))
    }

    /// Returns the target object followed by its display children in native
    /// insertion order. CDspObj setters at vtable +72/+76/+80 propagate over
    /// this hierarchy and retain the copied value when a child is detached.
    pub(crate) fn descendants_inclusive(&self, handle: i32) -> Vec<i32> {
        if !self.contains(handle) {
            return Vec::new();
        }
        let mut result = Vec::new();
        let mut pending = vec![handle];
        let mut visited = BTreeSet::new();
        while let Some(current) = pending.pop() {
            if !visited.insert(current) {
                continue;
            }
            result.push(current);
            if let Some(object) = self.objects.get(&current) {
                pending.extend(object.children.iter().rev().copied());
            }
        }
        result
    }

    pub(crate) fn set_enabled(&mut self, handle: i32, enabled: bool) {
        self.register_inferred(handle);
        if let Some(object) = self.objects.get_mut(&handle) {
            object.enabled = enabled;
        }
    }

    pub(crate) fn set_local_position(&mut self, handle: i32, x: f32, y: f32) {
        self.register_inferred(handle);
        if let Some(object) = self.objects.get_mut(&handle) {
            object.local_x = x;
            object.local_y = y;
        }
    }

    pub(crate) fn set_local_z(&mut self, handle: i32, z: i32) {
        self.register_inferred(handle);
        if let Some(object) = self.objects.get_mut(&handle) {
            object.local_z = z;
        }
    }

    pub(crate) fn set_transform(&mut self, handle: i32, x: f32, y: f32, z: i32) {
        self.register_inferred(handle);
        if let Some(object) = self.objects.get_mut(&handle) {
            object.transform_x = x;
            object.transform_y = y;
            object.transform_z = z;
        }
    }

    fn insert_chain_after_equal_depth(&mut self, handle: i32, depth: i32) {
        let native_sort_key = self
            .objects
            .get(&handle)
            .and_then(|object| object.manager_sort_key);
        if let Some(sort_key) = native_sort_key {
            // Once a target manager key is known, priority/depth changes must
            // preserve the target's secondary ordering inside the new band.
            self.display_chain.insert_after_equal_native_sort_key(
                &self.objects,
                handle,
                depth,
                sort_key,
            );
        } else {
            self.display_chain
                .insert_after_equal_depth(&self.objects, handle, depth);
        }
        if let Some(object) = self.objects.get_mut(&handle) {
            object.chain_depth = depth;
        }
    }

    /// Mirrors `DisplayChain_ReinsertByDepth`: remove the object and insert it
    /// after all existing objects with the same depth.
    pub(crate) fn set_chain_depth(&mut self, handle: i32, depth: i32) {
        self.register_inferred(handle);
        let changed = self
            .objects
            .get(&handle)
            .is_none_or(|object| object.chain_depth != depth);
        if changed {
            self.insert_chain_after_equal_depth(handle, depth);
        }
    }

    /// Reinsert one object using the target CObjectManager node key sampled
    /// from CDspObj vtable+0x1C. This is the operation performed by
    /// sub_443300 -> sub_4307D0 when an animation changes that key. Raw
    /// priority remains the primary portable render depth; the native key
    /// refines ordering inside that priority band without inventing a second
    /// renderer hierarchy.
    pub(crate) fn set_native_sort_key(&mut self, handle: i32, sort_key: u32) {
        self.register_inferred(handle);
        let Some(depth) = self.objects.get(&handle).map(|object| object.chain_depth) else {
            return;
        };
        let changed = self
            .objects
            .get(&handle)
            .is_none_or(|object| object.manager_sort_key != Some(sort_key));
        if !changed {
            return;
        }
        if let Some(object) = self.objects.get_mut(&handle) {
            object.manager_sort_key = Some(sort_key);
        }
        self.display_chain.insert_after_equal_native_sort_key(
            &self.objects,
            handle,
            depth,
            sort_key,
        );
    }

    pub(crate) fn native_sort_key(&self, handle: i32) -> Option<u32> {
        self.objects.get(&handle).and_then(|object| object.manager_sort_key)
    }

    /// Position in the target-style ascending depth chain.  The target
    /// insertion helper is confirmed to place equal-depth objects after
    /// existing nodes.  Traversal direction is kept separate and is not
    /// encoded into this structural index.
    pub(crate) fn chain_position(&self, handle: i32) -> Option<u64> {
        self.display_chain.position(handle)
    }

    pub(crate) fn chain_handles(&self) -> &[i32] {
        self.display_chain.handles()
    }

    pub(crate) fn creation_serial(&self, handle: i32) -> Option<u64> {
        self.objects
            .get(&handle)
            .map(|object| object.creation_serial)
    }

    /// Returns the transform contributed by ancestors only. The current
    /// object's own position is already materialized into existing layers by
    /// the compatibility graph code, so including it here would double-apply
    /// object movement while the migration is in progress.
    pub(crate) fn ancestor_transform(&self, handle: i32) -> (f32, f32, i32) {
        let mut x = 0.0;
        let mut y = 0.0;
        let mut z = 0_i32;
        let mut current = self.parent(handle);
        let mut visited = BTreeSet::new();
        while let Some(parent) = current {
            if !visited.insert(parent) {
                break;
            }
            let Some(object) = self.objects.get(&parent) else {
                break;
            };
            x += object.local_x + object.transform_x;
            y += object.local_y + object.transform_y;
            z = z
                .saturating_add(object.local_z)
                .saturating_add(object.transform_z);
            current = object.parent;
        }
        (x, y, z)
    }

    pub(crate) fn chain_drawable(&self, handle: i32) -> bool {
        let mut current = Some(handle);
        let mut visited = BTreeSet::new();
        while let Some(id) = current {
            if !visited.insert(id) {
                return false;
            }
            let Some(object) = self.objects.get(&id) else {
                return true;
            };
            if !object.drawable() {
                return false;
            }
            current = object.parent;
        }
        true
    }

    /// Returns the portable depth used while the target packed-depth fields
    /// are still unrecovered.  The display chain itself is authoritative for
    /// equal-depth ordering; this function only adds ancestor z propagation.
    pub(crate) fn effective_depth(&self, handle: i32, fallback_depth: i32) -> i32 {
        let (_, _, ancestor_z) = self.ancestor_transform(handle);
        // The exact packed CDspObj depth formula is not yet recovered.  Do not
        // invent band/subtype bits; preserve the script/runtime depth and only
        // add the target-confirmed ancestor transform contribution.
        fallback_depth.saturating_add(ancestor_z)
    }
}

#[cfg(test)]
mod tests {
    use super::{NativeDisplayKind, NativeDisplayTree};

    #[test]
    fn root_handle_is_a_real_display_object() {
        let tree = NativeDisplayTree::default();
        assert!(tree.contains(0));
        assert!(tree.chain_drawable(0));
    }

    #[test]
    fn invalid_minus_one_handle_is_never_registered() {
        let mut tree = NativeDisplayTree::default();
        assert_eq!(tree.register_inferred(-1), 0);
        assert!(!tree.contains(-1));
    }

    #[test]
    fn native_handle_tags_decode_without_signed_integer_loss() {
        assert_eq!(
            NativeDisplayKind::from_handle(0x8000_0001_u32 as i32),
            NativeDisplayKind::Sprite
        );
        assert_eq!(
            NativeDisplayKind::from_handle(0xb000_0002_u32 as i32),
            NativeDisplayKind::Window
        );
        assert_eq!(
            NativeDisplayKind::from_handle(0xf100_0003_u32 as i32),
            NativeDisplayKind::Group
        );
    }

    #[test]
    fn parent_chain_propagates_transform_and_visibility() {
        let mut tree = NativeDisplayTree::default();
        tree.register(10, NativeDisplayKind::Group);
        tree.register(11, NativeDisplayKind::Sprite);
        tree.set_local_position(10, 40.0, 25.0);
        tree.set_transform(10, 3.0, -2.0, 7);
        tree.set_parent(11, 10).unwrap();
        assert_eq!(tree.ancestor_transform(11), (43.0, 23.0, 7));
        assert!(tree.chain_drawable(11));
        tree.set_enabled(10, false);
        assert!(!tree.chain_drawable(11));
    }

    #[test]
    fn effective_depth_adds_ancestor_z_without_double_applying_child_z() {
        let mut tree = NativeDisplayTree::default();
        tree.register(10, NativeDisplayKind::Group);
        tree.register(11, NativeDisplayKind::Sprite);
        tree.set_local_z(10, 7);
        tree.set_local_z(11, 13);
        tree.set_parent(11, 10).unwrap();
        // The compatibility layer already supplies the child's own z as the
        // fallback. Only the ancestor contribution is added by the tree.
        assert_eq!(tree.effective_depth(11, 100), 107);
    }

    #[test]
    fn parent_cycle_is_rejected() {
        let mut tree = NativeDisplayTree::default();
        tree.register(1, NativeDisplayKind::Group);
        tree.register(2, NativeDisplayKind::Sprite);
        tree.set_parent(2, 1).unwrap();
        assert_eq!(tree.set_parent(1, 2), Err(13));
    }

    #[test]
    fn descendants_follow_native_child_insertion_order() {
        let mut tree = NativeDisplayTree::default();
        tree.register(1, NativeDisplayKind::Group);
        tree.register(2, NativeDisplayKind::Sprite);
        tree.register(3, NativeDisplayKind::Group);
        tree.register(4, NativeDisplayKind::Sprite);
        tree.set_parent(2, 1).unwrap();
        tree.set_parent(3, 1).unwrap();
        tree.set_parent(4, 3).unwrap();

        assert_eq!(tree.descendants_inclusive(1), vec![1, 2, 3, 4]);
        tree.set_parent(3, 0).unwrap();
        assert_eq!(tree.descendants_inclusive(1), vec![1, 2]);
        assert_eq!(tree.descendants_inclusive(3), vec![3, 4]);
    }

    #[test]
    fn display_chain_inserts_equal_depth_after_existing_nodes() {
        let mut tree = NativeDisplayTree::default();
        tree.register(10, NativeDisplayKind::Sprite);
        tree.register(11, NativeDisplayKind::Sprite);
        tree.register(12, NativeDisplayKind::Sprite);
        tree.set_chain_depth(10, 7);
        tree.set_chain_depth(11, 7);
        tree.set_chain_depth(12, 5);
        assert_eq!(tree.chain_handles(), &[0, 12, 10, 11]);
        assert!(tree.chain_position(11) > tree.chain_position(10));
    }

    #[test]
    fn native_manager_key_refines_equal_priority_chain_order() {
        let mut tree = NativeDisplayTree::default();
        tree.register(10, NativeDisplayKind::Sprite);
        tree.register(11, NativeDisplayKind::Sprite);
        tree.register(12, NativeDisplayKind::Sprite);
        tree.set_chain_depth(10, 7);
        tree.set_chain_depth(11, 7);
        tree.set_chain_depth(12, 7);

        tree.set_native_sort_key(10, 0x70020);
        tree.set_native_sort_key(11, 0x70010);
        tree.set_native_sort_key(12, 0x70020);
        // CObjectManager inserts ascending by unsigned key and places an
        // equal key after nodes already carrying that key.
        assert_eq!(tree.chain_handles(), &[0, 11, 10, 12]);

        tree.set_native_sort_key(12, 0x70008);
        assert_eq!(tree.chain_handles(), &[0, 12, 11, 10]);
    }

    #[test]
    fn depth_change_reinserts_instead_of_changing_creation_order() {
        let mut tree = NativeDisplayTree::default();
        tree.register(10, NativeDisplayKind::Sprite);
        tree.register(11, NativeDisplayKind::Sprite);
        let serial = tree.creation_serial(10);
        tree.set_chain_depth(10, 9);
        tree.set_chain_depth(11, 3);
        tree.set_chain_depth(10, 1);
        assert_eq!(tree.creation_serial(10), serial);
        assert_eq!(tree.chain_handles(), &[0, 10, 11]);
    }
}

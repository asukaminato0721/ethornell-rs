use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GraphSpriteTarget {
    pub target_number: i32,
    pub sprite_handle: i32,
    pub sampled_primary_state: i32,
}

#[derive(Debug, Default)]
pub(crate) struct GraphSpriteTargetRegistry {
    next_target_number: i32,
    targets: VecDeque<GraphSpriteTarget>,
}

impl GraphSpriteTargetRegistry {
    pub(crate) fn clear(&mut self) {
        self.targets.clear();
        self.next_target_number = 0;
    }

    pub(crate) fn register(&mut self, sprite_handle: i32) -> i32 {
        let target_number = self.next_target_number;
        self.next_target_number = self.next_target_number.saturating_add(1);
        self.targets.push_front(GraphSpriteTarget {
            target_number,
            sprite_handle,
            sampled_primary_state: 0,
        });
        target_number
    }

    pub(crate) fn unregister_sprite(&mut self, sprite_handle: i32) -> bool {
        let Some(index) = self
            .targets
            .iter()
            .position(|target| target.sprite_handle == sprite_handle)
        else {
            return false;
        };
        self.targets.remove(index);
        true
    }

    pub(crate) fn entries(&self) -> Vec<(i32, i32)> {
        self.targets
            .iter()
            .map(|target| (target.target_number, target.sprite_handle))
            .collect()
    }

    pub(crate) fn target_at_point(&self, mut is_hit: impl FnMut(i32) -> bool) -> i32 {
        self.targets
            .iter()
            .find(|target| is_hit(target.sprite_handle))
            .map(|target| target.target_number)
            .unwrap_or(-1)
    }

    pub(crate) fn sampled_state(&self, target_number: i32) -> Option<i32> {
        self.targets
            .iter()
            .find(|target| target.target_number == target_number)
            .map(|target| target.sampled_primary_state)
    }

    pub(crate) fn sprite_handles(&self) -> Vec<i32> {
        self.targets
            .iter()
            .map(|target| target.sprite_handle)
            .collect()
    }

    pub(crate) fn set_sampled_state(&mut self, sprite_handle: i32, state: i32) {
        for target in &mut self.targets {
            if target.sprite_handle == sprite_handle {
                target.sampled_primary_state = state & 1;
            }
        }
    }

    pub(crate) fn sample(&mut self, mut sample_sprite: impl FnMut(i32) -> i32) {
        for target in &mut self.targets {
            target.sampled_primary_state = sample_sprite(target.sprite_handle) & 1;
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.targets.len()
    }
}

#[cfg(test)]
mod tests {
    use super::GraphSpriteTargetRegistry;

    #[test]
    fn newest_registration_wins_hit_testing_and_ids_are_monotonic() {
        let mut registry = GraphSpriteTargetRegistry::default();
        assert_eq!(registry.register(10), 0);
        assert_eq!(registry.register(20), 1);
        assert_eq!(registry.target_at_point(|_| true), 1);
        registry.clear();
        assert_eq!(registry.register(30), 0);
    }
}

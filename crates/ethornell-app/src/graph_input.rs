use ethornell_vm::{GraphInputDescriptor, GraphInputRegion};
use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub(crate) struct RuntimeGraphInputObject {
    pub(crate) layer: i32,
    pub(crate) registered_state: i32,
    pub(crate) descriptor: GraphInputDescriptor,
    pub(crate) hovered: Option<(i32, i32)>,
    pub(crate) queued_events: VecDeque<[i32; 3]>,
    active: bool,
    completed_state: Option<[i32; 6]>,
}

impl RuntimeGraphInputObject {
    pub(crate) fn new(layer: i32) -> Self {
        Self {
            layer,
            registered_state: 0,
            descriptor: GraphInputDescriptor::default(),
            hovered: None,
            queued_events: VecDeque::new(),
            active: false,
            completed_state: None,
        }
    }

    pub(crate) fn configure(&mut self, descriptor: GraphInputDescriptor) {
        if self.hovered.is_some_and(|key| {
            !descriptor
                .regions
                .iter()
                .any(|region| (region.group, region.index) == key)
        }) {
            self.hovered = None;
        }
        self.descriptor = descriptor;
        self.active = true;
        self.completed_state = None;
        self.queued_events.clear();
    }

    pub(crate) fn queue_event(&mut self, event: [i32; 3]) {
        self.queued_events.push_back(event);
    }

    pub(crate) fn pop_event(&mut self) -> Option<[i32; 3]> {
        self.queued_events.pop_front()
    }

    pub(crate) fn complete(&mut self, region: GraphInputRegion, local_x: i32, local_y: i32) {
        self.active = false;
        self.completed_state = Some([0, region.group, region.index, 1, local_x, local_y]);
    }

    pub(crate) fn state_record(&self, hit: Option<(GraphInputRegion, i32, i32)>) -> [i32; 6] {
        if let Some(completed) = self.completed_state {
            return completed;
        }
        if !self.active {
            return [0; 6];
        }
        if let Some((region, local_x, local_y)) = hit {
            return [1, region.group, region.index, 0, local_x, local_y];
        }
        [1, self.descriptor.initial_group, -1, 0, 0, 0]
    }

    pub(crate) fn hit_test(&self, point: (f32, f32)) -> Option<(GraphInputRegion, i32, i32)> {
        self.descriptor
            .regions
            .iter()
            .filter(|region| region.enabled_depth != 0)
            .filter_map(|region| {
                let left = region.x as f32;
                let top = region.y as f32;
                let right = left + region.width as f32;
                let bottom = top + region.height as f32;
                (point.0 >= left && point.0 < right && point.1 >= top && point.1 < bottom).then(
                    || {
                        (
                            *region,
                            (point.0 - left).round() as i32,
                            (point.1 - top).round() as i32,
                        )
                    },
                )
            })
            .max_by_key(|(region, _, _)| native_region_depth(region))
    }
}

fn native_region_depth(region: &GraphInputRegion) -> i32 {
    if region.flags & 0x10 != 0 {
        region.enabled_depth
    } else if region.flags & 0x02 != 0 {
        region.y
    } else {
        region.ordinal
    }
}

pub(crate) fn pack_words(high: i32, low: i32) -> i32 {
    ((high & 0xffff) << 16) | (low & 0xffff)
}

#[cfg(test)]
mod tests {
    use super::RuntimeGraphInputObject;
    use ethornell_vm::{GraphInputDescriptor, GraphInputRegion};

    fn region() -> GraphInputRegion {
        GraphInputRegion {
            group: 0,
            index: 1,
            ordinal: 0,
            enabled_depth: 1,
            selected: false,
            x: 10,
            y: 20,
            width: 100,
            height: 50,
            normal_resource: 0,
            selected_resource: 0,
            mask_resource: -1,
            flags: 0,
        }
    }

    #[test]
    fn native_state_stays_active_until_a_region_completes() {
        let mut input = RuntimeGraphInputObject::new(7);
        input.configure(GraphInputDescriptor {
            initial_group: 0,
            regions: vec![region()],
            ..GraphInputDescriptor::default()
        });

        assert_eq!(input.state_record(None), [1, 0, -1, 0, 0, 0]);
        assert_eq!(
            input.state_record(input.hit_test((20.0, 30.0))),
            [1, 0, 1, 0, 10, 10]
        );

        input.complete(region(), 10, 10);
        assert_eq!(input.state_record(None), [0, 0, 1, 1, 10, 10]);
    }
}

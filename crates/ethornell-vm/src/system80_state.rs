use std::collections::{BTreeMap, VecDeque};

pub(crate) const NATIVE_NOT_FOUND: i32 = i32::MIN + 1;
pub(crate) const NATIVE_BUSY: i32 = i32::MIN + 2;
pub(crate) const NATIVE_NOT_OWNER: i32 = i32::MIN + 3;
pub(crate) const NATIVE_UNAVAILABLE: i32 = i32::MIN + 4;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct StructuredHistoryRecord {
    pub(crate) values: [i32; 9],
    pub(crate) short_text: [String; 3],
    pub(crate) text: String,
    pub(crate) extended_text: String,
}

#[derive(Debug, Default)]
pub(crate) struct StructuredHistoryState {
    pub(crate) capacity: u32,
    pub(crate) records: VecDeque<StructuredHistoryRecord>,
}

impl StructuredHistoryState {
    pub(crate) fn reset(&mut self, capacity: i32) {
        self.records.clear();
        self.capacity = capacity as u32;
    }

    pub(crate) fn push(&mut self, record: StructuredHistoryRecord) {
        if self.capacity == 0 {
            return;
        }
        self.records.push_back(record);
        while self.records.len() > self.capacity as usize {
            self.records.pop_front();
        }
    }

    pub(crate) fn newest(&self, index: u32) -> Option<StructuredHistoryRecord> {
        let len = self.records.len();
        let index = usize::try_from(index).ok()?;
        (index < len).then(|| self.records[len - 1 - index].clone())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ExclusionWaiter {
    thread_id: i32,
    priority: u32,
}

#[derive(Debug)]
struct ExclusionSection {
    id: u32,
    capacity: u32,
    holders: Vec<i32>,
    waiters: Vec<ExclusionWaiter>,
}

#[derive(Debug)]
pub(crate) struct ExclusionRegistry {
    next_id: u32,
    by_name: BTreeMap<String, ExclusionSection>,
}

impl Default for ExclusionRegistry {
    fn default() -> Self {
        Self {
            next_id: 1,
            by_name: BTreeMap::new(),
        }
    }
}

impl ExclusionRegistry {
    pub(crate) fn create(&mut self, name: String, capacity: i32) -> i32 {
        if self.by_name.contains_key(&name) {
            return 0;
        }
        let id = self.next_id.max(1);
        self.next_id = id.wrapping_add(1).max(1);
        self.by_name.insert(
            name,
            ExclusionSection {
                id,
                capacity: capacity as u32,
                holders: Vec::new(),
                waiters: Vec::new(),
            },
        );
        1
    }

    pub(crate) fn delete(&mut self, name: &str) -> i32 {
        let Some(section) = self.by_name.get(name) else {
            return NATIVE_NOT_FOUND;
        };
        if !section.holders.is_empty() || !section.waiters.is_empty() {
            return NATIVE_BUSY;
        }
        self.by_name.remove(name);
        0
    }

    pub(crate) fn enqueue(&mut self, name: &str, thread_id: i32, priority: u32) -> Option<u32> {
        let section = self.by_name.get_mut(name)?;
        let insertion = section
            .waiters
            .iter()
            .position(|waiter| priority > waiter.priority)
            .unwrap_or(section.waiters.len());
        section.waiters.insert(
            insertion,
            ExclusionWaiter {
                thread_id,
                priority,
            },
        );
        Some(section.id)
    }

    pub(crate) fn try_acquire(&mut self, section_id: u32, thread_id: i32) -> i32 {
        let Some(section) = self
            .by_name
            .values_mut()
            .find(|section| section.id == section_id)
        else {
            return NATIVE_NOT_FOUND;
        };
        let can_acquire = section
            .waiters
            .first()
            .is_some_and(|waiter| waiter.thread_id == thread_id)
            && section.holders.len() < section.capacity as usize;
        if !can_acquire {
            return NATIVE_UNAVAILABLE;
        }
        section.waiters.remove(0);
        section.holders.insert(0, thread_id);
        0
    }

    pub(crate) fn release(&mut self, name: &str, thread_id: i32) -> i32 {
        let Some(section) = self.by_name.get_mut(name) else {
            return NATIVE_NOT_FOUND;
        };
        let Some(index) = section
            .holders
            .iter()
            .position(|holder| *holder == thread_id)
        else {
            return NATIVE_NOT_OWNER;
        };
        section.holders.remove(index);
        0
    }

    pub(crate) fn query_available(&self, name: &str, priority: u32) -> i32 {
        let Some(section) = self.by_name.get(name) else {
            return NATIVE_NOT_FOUND;
        };
        let preceding = section
            .waiters
            .iter()
            .take_while(|waiter| priority <= waiter.priority)
            .count();
        if section.capacity as usize >= section.holders.len().saturating_add(preceding) {
            0
        } else {
            NATIVE_UNAVAILABLE
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct System80SharedState {
    pub(crate) history: StructuredHistoryState,
    pub(crate) exclusions: ExclusionRegistry,
    pub(crate) system_mode_flag: u32,
}

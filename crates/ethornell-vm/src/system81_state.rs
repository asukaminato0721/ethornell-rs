use std::collections::{BTreeMap, VecDeque};

pub(crate) const NATIVE_INVALID_ARGUMENT: i32 = i32::MIN + 1;
pub(crate) const NATIVE_NOT_FOUND: i32 = i32::MIN + 2;
pub(crate) const NATIVE_OPERATION_FAILED: i32 = i32::MIN + 3;
pub(crate) const NATIVE_INVALID_INDEX: i32 = i32::MIN + 4;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PointerSample {
    pub(crate) x: i32,
    pub(crate) y: i32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TouchRecord {
    pub(crate) values: [i32; 6],
}

#[derive(Debug, Clone)]
pub(crate) struct ResourceStream {
    pub(crate) path: String,
    pub(crate) mode: i32,
    pub(crate) bytes: Vec<u8>,
    pub(crate) cursor: usize,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct BlobTable {
    pub(crate) slots: Vec<Option<Vec<u8>>>,
}

impl BlobTable {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            slots: vec![None; capacity],
        }
    }

    pub(crate) fn insert_first_free(&mut self, bytes: Vec<u8>) -> usize {
        if let Some(index) = self.slots.iter().position(Option::is_none) {
            self.slots[index] = Some(bytes);
            return index;
        }
        self.slots.push(Some(bytes));
        self.slots.len() - 1
    }

    pub(crate) fn insert_at(&mut self, index: usize, bytes: Vec<u8>) {
        if index >= self.slots.len() {
            self.slots.resize(index + 1, None);
        }
        self.slots[index] = Some(bytes);
    }
}

#[derive(Debug)]
pub(crate) struct System81SharedState {
    pub(crate) clock_jump_threshold_ms: i32,
    pub(crate) coordinate_slots: [Option<(i32, i32)>; 5],
    pub(crate) input_binding_values: [i32; 256],
    pub(crate) keyboard_polling_override: i32,
    /// Target dword_507690 shared by CProcedure/CProcDspMsg input filtering.
    pub(crate) message_auxiliary_input_mask: i32,
    pub(crate) pointer_history_capacity: usize,
    pub(crate) pointer_history_distance: f64,
    pub(crate) pointer_history: VecDeque<PointerSample>,
    pub(crate) touch_registered: bool,
    pub(crate) touch_records: Vec<TouchRecord>,
    pub(crate) controller_wake_entries: [i32; 36],
    pub(crate) resource_streams: BTreeMap<u32, ResourceStream>,
    pub(crate) next_resource_stream_handle: u32,
    pub(crate) blob_tables: BTreeMap<u32, BlobTable>,
    pub(crate) next_blob_table_handle: u32,
    pub(crate) named_mutexes: BTreeMap<u32, String>,
    pub(crate) mutex_names: BTreeMap<String, u32>,
    pub(crate) next_mutex_handle: u32,
    pub(crate) window_position_override: i32,
    pub(crate) pause_on_deactivate: i32,
    pub(crate) print_screen_hotkeys_enabled: bool,
    pub(crate) error_capture_mode: i32,
    pub(crate) captured_error: String,
    pub(crate) pixel_shader_version: u16,
}

impl Default for System81SharedState {
    fn default() -> Self {
        Self {
            clock_jump_threshold_ms: 1000,
            coordinate_slots: [None; 5],
            input_binding_values: [0; 256],
            keyboard_polling_override: 0,
            message_auxiliary_input_mask: 0,
            pointer_history_capacity: 0,
            pointer_history_distance: 0.0,
            pointer_history: VecDeque::new(),
            touch_registered: false,
            touch_records: Vec::new(),
            controller_wake_entries: [0; 36],
            resource_streams: BTreeMap::new(),
            next_resource_stream_handle: 1,
            blob_tables: BTreeMap::new(),
            next_blob_table_handle: 1,
            named_mutexes: BTreeMap::new(),
            mutex_names: BTreeMap::new(),
            next_mutex_handle: 1,
            window_position_override: 0,
            pause_on_deactivate: 0,
            print_screen_hotkeys_enabled: false,
            error_capture_mode: 0,
            captured_error: String::new(),
            pixel_shader_version: 0,
        }
    }
}

impl System81SharedState {
    pub(crate) fn configure_pointer_history(&mut self, capacity: i32, distance: i32) -> bool {
        if !(0..=512).contains(&capacity) {
            return false;
        }
        self.pointer_history_capacity = capacity as usize;
        self.pointer_history_distance = f64::from(distance);
        self.pointer_history.clear();
        true
    }

    pub(crate) fn record_pointer(&mut self, x: i32, y: i32) {
        if self.pointer_history_capacity == 0 {
            return;
        }
        let should_push = self.pointer_history.back().map_or(true, |previous| {
            let dx = f64::from(x.saturating_sub(previous.x));
            let dy = f64::from(y.saturating_sub(previous.y));
            (dx * dx + dy * dy).sqrt() >= self.pointer_history_distance
        });
        if should_push {
            self.pointer_history.push_front(PointerSample { x, y });
            self.pointer_history.truncate(self.pointer_history_capacity);
        }
    }

    pub(crate) fn open_resource_stream(
        &mut self,
        path: String,
        mode: i32,
        bytes: Vec<u8>,
    ) -> Result<u32, i32> {
        if !(0..=2).contains(&mode) {
            return Err(NATIVE_INVALID_ARGUMENT);
        }
        if self
            .resource_streams
            .values()
            .any(|stream| stream.path == path)
        {
            return Err(NATIVE_NOT_FOUND);
        }
        let handle = self.next_resource_stream_handle.max(1);
        self.next_resource_stream_handle = handle.wrapping_add(1).max(1);
        self.resource_streams.insert(
            handle,
            ResourceStream {
                path,
                mode,
                bytes,
                cursor: 0,
            },
        );
        Ok(handle)
    }

    pub(crate) fn create_blob_table(&mut self, capacity: i32) -> Result<u32, i32> {
        if capacity <= 1 {
            return Err(NATIVE_INVALID_ARGUMENT);
        }
        let handle = self.next_blob_table_handle.max(1);
        self.next_blob_table_handle = handle.wrapping_add(1).max(1);
        self.blob_tables
            .insert(handle, BlobTable::with_capacity(capacity as usize));
        Ok(handle)
    }

    pub(crate) fn create_named_mutex(&mut self, name: String) -> i32 {
        if name.is_empty() || self.mutex_names.contains_key(&name) {
            return 0;
        }
        let handle = self.next_mutex_handle.max(1);
        self.next_mutex_handle = handle.wrapping_add(1).max(1);
        self.mutex_names.insert(name.clone(), handle);
        self.named_mutexes.insert(handle, name);
        handle as i32
    }

    pub(crate) fn release_named_mutex(&mut self, handle: i32) -> bool {
        let Ok(handle) = u32::try_from(handle) else {
            return false;
        };
        let Some(name) = self.named_mutexes.remove(&handle) else {
            return false;
        };
        self.mutex_names.remove(&name);
        true
    }
}

pub(crate) fn md5_digest(input: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];

    let bit_len = (input.len() as u64).wrapping_mul(8);
    let mut message = input.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_le_bytes());

    let mut a0 = 0x67452301u32;
    let mut b0 = 0xefcdab89u32;
    let mut c0 = 0x98badcfeu32;
    let mut d0 = 0x10325476u32;

    for chunk in message.chunks_exact(64) {
        let mut words = [0u32; 16];
        for (index, word) in words.iter_mut().enumerate() {
            let start = index * 4;
            *word = u32::from_le_bytes(chunk[start..start + 4].try_into().unwrap());
        }
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for index in 0..64 {
            let (f, g) = match index {
                0..=15 => ((b & c) | ((!b) & d), index),
                16..=31 => ((d & b) | ((!d) & c), (5 * index + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * index + 5) % 16),
                _ => (c ^ (b | !d), (7 * index) % 16),
            };
            let next = a
                .wrapping_add(f)
                .wrapping_add(K[index])
                .wrapping_add(words[g])
                .rotate_left(S[index])
                .wrapping_add(b);
            a = d;
            d = c;
            c = b;
            b = next;
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }

    let mut digest = [0u8; 16];
    digest[0..4].copy_from_slice(&a0.to_le_bytes());
    digest[4..8].copy_from_slice(&b0.to_le_bytes());
    digest[8..12].copy_from_slice(&c0.to_le_bytes());
    digest[12..16].copy_from_slice(&d0.to_le_bytes());
    digest
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_matches_standard_vectors() {
        assert_eq!(
            md5_digest(b""),
            [
                0xd4, 0x1d, 0x8c, 0xd9, 0x8f, 0x00, 0xb2, 0x04, 0xe9, 0x80, 0x09, 0x98, 0xec, 0xf8,
                0x42, 0x7e,
            ]
        );
        assert_eq!(
            md5_digest(b"abc"),
            [
                0x90, 0x01, 0x50, 0x98, 0x3c, 0xd2, 0x4f, 0xb0, 0xd6, 0x96, 0x3f, 0x7d, 0x28, 0xe1,
                0x7f, 0x72,
            ]
        );
    }

    #[test]
    fn blob_slots_grow_and_reuse_first_free_entry() {
        let mut table = BlobTable::with_capacity(2);
        table.insert_at(1, vec![1, 2]);
        assert_eq!(table.insert_first_free(vec![3]), 0);
        assert_eq!(table.insert_first_free(vec![4, 5, 6]), 2);
        assert_eq!(table.slots[0].as_deref(), Some(&[3][..]));
        assert_eq!(table.slots[1].as_deref(), Some(&[1, 2][..]));
        assert_eq!(table.slots[2].as_deref(), Some(&[4, 5, 6][..]));
    }

    #[test]
    fn named_mutex_handles_are_monotonic_and_names_are_unique() {
        let mut state = System81SharedState::default();
        let first = state.create_named_mutex("BGI-test".into());
        assert!(first > 0);
        assert_eq!(state.create_named_mutex("BGI-test".into()), 0);
        assert!(state.release_named_mutex(first));
        let second = state.create_named_mutex("BGI-test".into());
        assert!(second > first);
    }
}

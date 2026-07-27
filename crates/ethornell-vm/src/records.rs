use crate::{Value, Vm, VmResult};

#[derive(Debug, Clone)]
pub(crate) struct RecordTableState {
    pub(crate) slot: u32,
    pub(crate) record_size: u32,
    pub(crate) cursor: u32,
    pub(crate) capacity: u32,
    pub(crate) keys: std::collections::BTreeMap<u32, Value>,
}

#[derive(Debug, Clone)]
pub(crate) struct IndexedRecordEntry {
    pub(crate) bytes: Vec<u8>,
    pub(crate) shadow_values: std::collections::BTreeMap<u32, Value>,
}

#[derive(Debug, Clone)]
pub(crate) struct IndexedRecordState {
    pub(crate) slot: u32,
    pub(crate) capacity: u32,
    pub(crate) record_size: u32,
    pub(crate) records: std::collections::VecDeque<IndexedRecordEntry>,
}

impl Vm {
    pub(crate) fn clear_shadow_values(&mut self, ptr: u32, size: usize) {
        if size == 0 {
            return;
        }
        self.trace_watch_write(ptr, size, 0, "clear_shadow_values");
        let start = Self::value_key(ptr);
        let end = start.saturating_add(size as u32);
        self.mem_values
            .retain(|addr, _| *addr < start || *addr >= end);
    }

    pub(crate) fn copy_shadow_values(&mut self, src: u32, dst: u32, size: usize) {
        if size == 0 {
            return;
        }
        self.clear_shadow_values(dst, size);
        let src_start = Self::value_key(src);
        let dst_start = Self::value_key(dst);
        let end = src_start.saturating_add(size as u32);
        let copied: Vec<_> = self
            .mem_values
            .iter()
            .filter_map(|(addr, value)| {
                if *addr >= src_start && *addr < end {
                    Some((dst_start.saturating_add(*addr - src_start), value.clone()))
                } else {
                    None
                }
            })
            .collect();
        self.mem_values.extend(copied);
    }

    pub(crate) fn normalize_scenario_descriptor_src(&self, src: u32, size: usize) -> u32 {
        if size != 100 || (src & 7) != 2 {
            return src;
        }
        let candidate = src.saturating_add(6);
        let Ok(current) = self.read_int(src, 2) else {
            return src;
        };
        let Ok(first) = self.read_int(candidate, 2) else {
            return src;
        };
        let Ok(second) = self.read_int(candidate.saturating_add(4), 2) else {
            return src;
        };
        if current > 0x00ff_ffff && first <= 0x1000 && second <= 0x10000 {
            tracing::debug!(
                src = format_args!("0x{src:08X}"),
                adjusted = format_args!("0x{candidate:08X}"),
                first = format_args!("0x{first:08X}"),
                second = format_args!("0x{second:08X}"),
                "ScenarioDescriptorCopyAlign"
            );
            candidate
        } else {
            src
        }
    }

    pub(crate) fn remember_script_record(&mut self, ptr: u32, text: &str) -> VmResult<()> {
        self.write_c_string_raw(ptr, text)?;
        self.script_records
            .insert(Self::value_key(ptr), text.to_string());
        self.mem_values
            .insert(Self::value_key(ptr), Value::Str(text.to_string()));
        Ok(())
    }

    pub(crate) fn restore_script_records(&mut self, ptr: u32, size: usize) -> VmResult<()> {
        if size == 0 || self.script_records.is_empty() {
            return Ok(());
        }
        let start = Self::value_key(ptr);
        let end = start.saturating_add(size as u32);
        let records: Vec<_> = self
            .script_records
            .iter()
            .filter_map(|(addr, text)| {
                (*addr >= start && *addr < end).then(|| (*addr, text.clone()))
            })
            .collect();
        for (addr, text) in records {
            self.write_c_string_raw(addr, &text)?;
            self.mem_values.insert(addr, Value::Str(text));
        }
        Ok(())
    }

    pub(crate) fn decode_sdc_records(&mut self, src: u32, dst: u32) -> VmResult<i32> {
        const HEADER_SIZE: usize = 32;
        const MAGIC: &[u8; 16] = b"SDC FORMAT 1.00\0";

        let header = self.resolve_range(src, HEADER_SIZE)?;
        let header = self.memory[header].to_vec();
        if header.get(..MAGIC.len()) != Some(MAGIC) {
            return Ok(0);
        }

        let seed = u32::from_le_bytes(header[16..20].try_into().unwrap());
        let packed_len = u32::from_le_bytes(header[20..24].try_into().unwrap()) as usize;
        let expected_sum = u16::from_le_bytes(header[28..30].try_into().unwrap());
        let expected_xor = u16::from_le_bytes(header[30..32].try_into().unwrap());
        let packed = self.resolve_range(src.saturating_add(HEADER_SIZE as u32), packed_len)?;
        let mut packed = self.memory[packed].to_vec();

        let sum = packed
            .iter()
            .fold(0u16, |sum, byte| sum.wrapping_add(u16::from(*byte)));
        let xor = packed.iter().fold(0u16, |xor, byte| xor ^ u16::from(*byte));
        if sum != expected_sum || xor != expected_xor {
            tracing::warn!(
                sum = format_args!("0x{sum:04X}"),
                expected_sum = format_args!("0x{expected_sum:04X}"),
                xor = format_args!("0x{xor:04X}"),
                expected_xor = format_args!("0x{expected_xor:04X}"),
                "SDC checksum mismatch"
            );
            return Ok(0);
        }

        let mut state = seed;
        for byte in &mut packed {
            let low = state as u16 as u32;
            let high = (state >> 16) as u16 as u32;
            let low_product = low * 0x4e35;
            let next_high = high
                .wrapping_mul(0x4e35)
                .wrapping_add(state.wrapping_mul(0x015a))
                .wrapping_add(low_product >> 16)
                & 0xffff;
            let next_low = (low_product as u16).wrapping_add(1);
            state = (next_high << 16) | u32::from(next_low);
            let random = (next_high & 0x7fff) as u8;
            *byte = byte.wrapping_sub(random);
        }

        let unpacked = unpack_sdc_lz(&packed)?;
        let range = self.resolve_write_range(dst, unpacked.len())?;
        self.memory[range].copy_from_slice(&unpacked);
        self.clear_shadow_values(dst, unpacked.len());
        Ok(unpacked.len() as i32)
    }

    pub(crate) fn decode_sdc_struct_array(&mut self, src: u32, dst: u32) -> VmResult<i32> {
        const STRUCT_MAGIC: &[u8; 16] = b"DCFS FORMAT 1.00";

        let decoded_len = self.read_int(src.saturating_add(24), 2)? as usize;
        if decoded_len < 24 {
            return Ok(0);
        }
        let temporary = self.alloc_heap(decoded_len as u32);
        if self.decode_sdc_records(src, temporary)? as usize != decoded_len {
            return Ok(0);
        }
        let range = self.resolve_range(temporary, decoded_len)?;
        let encoded = self.memory[range].to_vec();
        if encoded.get(..STRUCT_MAGIC.len()) != Some(STRUCT_MAGIC) {
            return Ok(0);
        }

        let record_size = u32::from_le_bytes(encoded[16..20].try_into().unwrap()) as usize;
        let record_count = u32::from_le_bytes(encoded[20..24].try_into().unwrap()) as usize;
        if record_size == 0 || record_count == 0 {
            return Ok(0);
        }
        let first_end = 24usize.saturating_add(record_size);
        let first = encoded
            .get(24..first_end)
            .ok_or_else(|| crate::VmError::Runtime("truncated DCFS first record".into()))?;
        let mut records = Vec::with_capacity(record_size.saturating_mul(record_count));
        records.extend_from_slice(first);
        let mut input = first_end;

        for _ in 1..record_count {
            let previous = records[records.len() - record_size..].to_vec();
            let mut record = Vec::with_capacity(record_size);
            let mut literal = false;
            while record.len() < record_size {
                let count = read_dcfs_varint(&encoded, &mut input)?;
                if record.len().saturating_add(count) > record_size {
                    return Err(crate::VmError::Runtime("invalid DCFS run length".into()));
                }
                if literal {
                    let end = input.saturating_add(count);
                    let bytes = encoded
                        .get(input..end)
                        .ok_or_else(|| crate::VmError::Runtime("truncated DCFS literal".into()))?;
                    record.extend_from_slice(bytes);
                    input = end;
                } else {
                    let start = record.len();
                    record.extend_from_slice(&previous[start..start + count]);
                }
                literal = !literal;
            }
            records.extend_from_slice(&record);
        }

        let range = self.resolve_write_range(dst, records.len())?;
        self.memory[range].copy_from_slice(&records);
        self.clear_shadow_values(dst, records.len());
        Ok(record_count as i32)
    }

    pub(crate) fn sys_record_table_open(&mut self, slot: u32, record_size: u32) -> VmResult<Value> {
        let capacity = if slot == 1972 && record_size == 124 {
            32
        } else {
            4096
        };
        let ptr = if slot == 1972 && record_size == 124 {
            0x0004_cad8
        } else {
            self.alloc_heap(record_size.saturating_mul(capacity).saturating_add(4))
        };
        self.write_value(slot, 2, &Value::Ptr(ptr))?;
        self.record_tables.insert(
            Self::value_key(ptr),
            RecordTableState {
                slot,
                record_size,
                cursor: 0,
                capacity,
                keys: std::collections::BTreeMap::new(),
            },
        );
        if std::env::var_os("DEBUG").is_some() {
            tracing::info!(
                slot = format_args!("0x{slot:08X}"),
                record_size,
                capacity,
                ptr = format_args!("0x{ptr:08X}"),
                "RecordTableOpen"
            );
        }
        Ok(Value::Ptr(ptr))
    }

    pub(crate) fn sys_record_table_copy(
        &mut self,
        handle: u32,
        key: Value,
        src: u32,
    ) -> VmResult<()> {
        let handle_addr = Self::value_key(handle);
        let key = self.normalize_record_key(key);
        if let Some((index, slot, record_size, capacity)) =
            self.record_tables.get_mut(&handle_addr).map(|table| {
                let existing = table
                    .keys
                    .iter()
                    .find_map(|(index, saved)| record_keys_equal(saved, &key).then_some(*index));
                let index = existing.unwrap_or(table.cursor);
                if existing.is_none() {
                    table.cursor = table.cursor.saturating_add(1);
                }
                table.keys.insert(index, key.clone());
                (index, table.slot, table.record_size, table.capacity)
            })
        {
            if std::env::var_os("DEBUG").is_some() {
                tracing::debug!(
                    index,
                    slot = format_args!("0x{slot:08X}"),
                    record_size,
                    capacity,
                    handle = format_args!("0x{handle:08X}"),
                    key = ?key,
                    src = format_args!("0x{src:08X}"),
                    "RecordTableCopy"
                );
            }
            if record_size > 0 {
                let table_dst = handle_addr.saturating_add(index.saturating_mul(record_size));
                self.copy_record_into_table(table_dst, src, record_size as usize)?;
            }
        } else if let Some(dst) = value_ptr(&key) {
            if dst as usize + 4 <= self.memory.len() && src as usize + 4 <= self.memory.len() {
                let value = self.read_int(src, 2)?;
                self.write_int(dst, 2, value)?;
            }
        }
        Ok(())
    }

    pub(crate) fn sys_record_table_close(&mut self, handle: u32) {
        self.record_tables.remove(&Self::value_key(handle));
    }

    pub(crate) fn sys_record_table_fetch(
        &mut self,
        dst: u32,
        handle: u32,
        selector: Value,
        mode: i32,
    ) -> VmResult<Value> {
        let handle_addr = Self::value_key(handle);
        let Some(table) = self.record_tables.get(&handle_addr).cloned() else {
            return Ok(Value::Int(1));
        };
        if table.record_size == 0 {
            return Ok(Value::Int(1));
        }

        let Some(index) = self.find_record_index(handle_addr, &table, &selector, mode)? else {
            if std::env::var_os("DEBUG").is_some() {
                tracing::debug!(
                    slot = format_args!("0x{:08X}", table.slot),
                    handle = format_args!("0x{handle:08X}"),
                    selector = ?selector,
                    mode,
                    "RecordTableFetchMiss"
                );
            }
            return Ok(Value::Int(1));
        };

        let src = handle_addr.saturating_add(index.saturating_mul(table.record_size));
        self.copy_buffer(dst, src, table.record_size as usize)?;
        if std::env::var_os("DEBUG").is_some() {
            let descriptor = (table.record_size >= 8).then(|| {
                (
                    self.read_int(src, 2).unwrap_or_default(),
                    self.read_int(src.saturating_add(4), 2).unwrap_or_default(),
                )
            });
            tracing::debug!(
                index,
                slot = format_args!("0x{:08X}", table.slot),
                record_size = table.record_size,
                handle = format_args!("0x{handle:08X}"),
                src = format_args!("0x{src:08X}"),
                dst = format_args!("0x{dst:08X}"),
                selector = ?selector,
                mode,
                descriptor = ?descriptor,
                "RecordTableFetch"
            );
        }
        Ok(Value::Int(0))
    }

    pub(crate) fn sys_indexed_record_open(
        &mut self,
        slot: u32,
        capacity: u32,
        record_size: u32,
    ) -> VmResult<i32> {
        if capacity == 0 || record_size == 0 {
            return Ok(2);
        }
        let handle = self.alloc_heap(4);
        self.write_value(slot, 2, &Value::Ptr(handle))?;
        self.indexed_record_tables.insert(
            Self::value_key(handle),
            IndexedRecordState {
                slot,
                capacity,
                record_size,
                records: std::collections::VecDeque::new(),
            },
        );

        self.ensure_indexed_record_globals(record_size)?;

        if std::env::var_os("DEBUG").is_some() {
            tracing::info!(
                slot = format_args!("0x{slot:08X}"),
                capacity,
                record_size,
                handle = format_args!("0x{handle:08X}"),
                "IndexedRecordOpen"
            );
        }
        Ok(0)
    }

    pub(crate) fn sys_indexed_record_close(&mut self, handle: u32) -> i32 {
        if let Some(state) = self.indexed_record_tables.remove(&Self::value_key(handle)) {
            if std::env::var_os("DEBUG").is_some() {
                tracing::info!(
                    handle = format_args!("0x{handle:08X}"),
                    slot = format_args!("0x{:08X}", state.slot),
                    capacity = state.capacity,
                    record_size = state.record_size,
                    "IndexedRecordClose"
                );
            }
            0
        } else {
            1
        }
    }

    pub(crate) fn sys_indexed_record_count(&mut self, dst: u32, handle: u32) -> VmResult<i32> {
        let Some(state) = self.indexed_record_tables.get(&Self::value_key(handle)) else {
            return Ok(1);
        };
        let count = state.records.len() as u32;
        self.write_int(dst, 2, count)?;
        if std::env::var_os("DEBUG").is_some() {
            tracing::info!(
                dst = format_args!("0x{dst:08X}"),
                handle = format_args!("0x{handle:08X}"),
                count,
                "IndexedRecordCount"
            );
        }
        Ok(0)
    }

    pub(crate) fn sys_indexed_record_push(&mut self, handle: u32, src: u32) -> VmResult<i32> {
        let key = Self::value_key(handle);
        let Some(state) = self.indexed_record_tables.get(&key) else {
            return Ok(1);
        };
        let size = state.record_size as usize;
        let capacity = state.capacity as usize;
        let src_range = self.resolve_range(src, size)?;
        let src_start = Self::value_key(src);
        let src_end = src_start.saturating_add(state.record_size);
        let shadow_values = self
            .mem_values
            .iter()
            .filter_map(|(addr, value)| {
                (*addr >= src_start && *addr < src_end).then(|| (*addr - src_start, value.clone()))
            })
            .collect();
        let entry = IndexedRecordEntry {
            bytes: self.memory[src_range].to_vec(),
            shadow_values,
        };
        let state = self.indexed_record_tables.get_mut(&key).unwrap();
        state.records.push_front(entry);
        state.records.truncate(capacity);
        Ok(0)
    }

    pub(crate) fn sys_indexed_record_load(
        &mut self,
        dst: u32,
        handle: u32,
        index: u32,
    ) -> VmResult<i32> {
        let Some(state) = self.indexed_record_tables.get(&Self::value_key(handle)) else {
            return Ok(1);
        };
        let Some(entry) = state.records.get(index as usize).cloned() else {
            return Ok(2);
        };
        let size = state.record_size as usize;
        let record_size = state.record_size;
        let capacity = state.capacity;
        self.ensure_writable(dst, size);
        if Self::should_clear_indexed_record_target(dst) {
            self.clear_record_buffer(dst, size);
        }
        let start = Self::memory_addr(dst) as usize;
        self.memory[start..start + size].copy_from_slice(&entry.bytes);
        self.clear_shadow_values(dst, size);
        let dst_start = Self::value_key(dst);
        self.mem_values.extend(
            entry
                .shadow_values
                .into_iter()
                .map(|(offset, value)| (dst_start.saturating_add(offset), value)),
        );

        if std::env::var_os("DEBUG").is_some() {
            let msg_base = dst.saturating_add(286_276);
            let msg_valid = self
                .read_int(msg_base.saturating_add(12), 2)
                .unwrap_or_default();
            let msg_count = self
                .read_int(msg_base.saturating_add(16), 2)
                .unwrap_or_default();
            tracing::info!(
                dst = format_args!("0x{dst:08X}"),
                handle = format_args!("0x{handle:08X}"),
                index,
                record_size,
                capacity,
                msg_base = format_args!("0x{msg_base:08X}"),
                msg_valid,
                msg_count,
                "IndexedRecordLoad"
            );
        }
        Ok(0)
    }

    pub(crate) fn sys_indexed_record_remove(&mut self, handle: u32, start: u32, count: u32) -> i32 {
        let Some(state) = self.indexed_record_tables.get_mut(&Self::value_key(handle)) else {
            return 1;
        };
        let start = start as usize;
        if start >= state.records.len() {
            return 2;
        }
        let end = start
            .saturating_add(count as usize)
            .min(state.records.len());
        state.records.drain(start..end);
        0
    }

    fn ensure_indexed_record_globals(&mut self, record_size: u32) -> VmResult<()> {
        const PRIMARY_BASE: u32 = 0x0004_cad8;
        let Some(total) = record_size.checked_mul(2) else {
            return Ok(());
        };
        self.ensure_writable(PRIMARY_BASE, total as usize);
        Ok(())
    }

    fn ensure_writable(&mut self, ptr: u32, size: usize) {
        let start = Self::memory_addr(ptr) as usize;
        let end = start.saturating_add(size);
        if end > self.memory.len() {
            self.memory.resize(end, 0);
        }
    }

    fn clear_record_buffer(&mut self, ptr: u32, size: usize) {
        if size == 0 {
            return;
        }
        self.ensure_writable(ptr, size);
        let start = Self::memory_addr(ptr) as usize;
        let end = start.saturating_add(size);
        self.memory[start..end].fill(0);
        self.clear_shadow_values(ptr, size);
        self.trace_watch_write(ptr, size, 0, "clear_record_buffer");
    }

    fn should_clear_indexed_record_target(ptr: u32) -> bool {
        const PRIMARY_GLOBAL_RECORD: u32 = 0x0004_cad8;
        Self::memory_addr(ptr) != PRIMARY_GLOBAL_RECORD
    }

    fn copy_record_into_table(&mut self, dst: u32, src: u32, size: usize) -> VmResult<()> {
        let end = Self::memory_addr(dst) as usize + size;
        if end > self.memory.len() {
            self.memory.resize(end, 0);
        }
        self.copy_buffer(dst, src, size)
    }

    fn find_record_index(
        &self,
        handle_addr: u32,
        table: &RecordTableState,
        selector: &Value,
        mode: i32,
    ) -> VmResult<Option<u32>> {
        let selector = self.normalize_record_key(selector.clone());
        if matches!(selector, Value::Int(0) | Value::Ptr(0) | Value::None)
            && mode >= 0
            && (mode as u32) < table.cursor
        {
            return Ok(Some(mode as u32));
        }
        if let Some(index) = table
            .keys
            .iter()
            .find_map(|(index, key)| record_keys_equal(key, &selector).then_some(*index))
        {
            return Ok(Some(index));
        }

        let selector_text = match &selector {
            Value::Str(text) => Some(text.clone()),
            Value::Ptr(ptr) => self
                .read_c_string(*ptr)
                .ok()
                .filter(|text| !text.is_empty()),
            Value::Int(value) if *value != 0 => self
                .read_c_string(*value as u32)
                .ok()
                .filter(|text| !text.is_empty()),
            _ => None,
        };
        let selector_int = selector.as_i32();

        for index in 0..table.cursor {
            let record = handle_addr.saturating_add(index.saturating_mul(table.record_size));
            if let Some(text) = selector_text.as_deref() {
                if self.record_matches_text(record, table.record_size, text)? {
                    return Ok(Some(index));
                }
            }
            if selector_int != 0
                && self.record_matches_int(record, table.record_size, selector_int)?
            {
                return Ok(Some(index));
            }
        }
        Ok(None)
    }

    fn normalize_record_key(&self, key: Value) -> Value {
        match key {
            Value::Ptr(ptr) => self
                .read_c_string(ptr)
                .ok()
                .filter(|text| !text.is_empty())
                .map(Value::Str)
                .unwrap_or(Value::Ptr(ptr)),
            Value::Int(ptr) if ptr != 0 => self
                .read_c_string(ptr as u32)
                .ok()
                .filter(|text| !text.is_empty())
                .map(Value::Str)
                .unwrap_or(Value::Int(ptr)),
            other => other,
        }
    }

    fn record_matches_text(&self, record: u32, record_size: u32, text: &str) -> VmResult<bool> {
        if let Some(Value::Str(value)) = self.mem_values.get(&Self::value_key(record)) {
            if value == text {
                return Ok(true);
            }
        }
        let max_offset = record_size.saturating_sub(4);
        for offset in (0..=max_offset).step_by(4) {
            let ptr = self.read_int(record.saturating_add(offset), 2)?;
            if ptr != 0 {
                if let Ok(value) = self.read_c_string(ptr) {
                    if value == text {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }

    fn record_matches_int(&self, record: u32, record_size: u32, needle: i32) -> VmResult<bool> {
        let max_offset = record_size.saturating_sub(4);
        for offset in (0..=max_offset).step_by(4) {
            if self.read_int(record.saturating_add(offset), 2)? as i32 == needle {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

fn unpack_sdc_lz(packed: &[u8]) -> VmResult<Vec<u8>> {
    let mut input = 0usize;
    let mut output = Vec::new();
    while input < packed.len() {
        let control = packed[input];
        input += 1;
        if control & 0x80 == 0 {
            let count = control as usize + 1;
            let end = input.saturating_add(count);
            let bytes = packed
                .get(input..end)
                .ok_or_else(|| crate::VmError::Runtime("truncated SDC literal".into()))?;
            output.extend_from_slice(bytes);
            input = end;
        } else {
            let distance_low = *packed
                .get(input)
                .ok_or_else(|| crate::VmError::Runtime("truncated SDC back-reference".into()))?;
            input += 1;
            let count = ((control >> 3) & 0x0f) as usize + 2;
            let distance = (((control & 7) as usize) << 8) + distance_low as usize + 2;
            if distance > output.len() {
                return Err(crate::VmError::Runtime("invalid SDC back-reference".into()));
            }
            for _ in 0..count {
                let byte = output[output.len() - distance];
                output.push(byte);
            }
        }
    }
    Ok(output)
}

fn read_dcfs_varint(encoded: &[u8], input: &mut usize) -> VmResult<usize> {
    let mut value = 0usize;
    let mut shift = 0u32;
    loop {
        let byte = *encoded
            .get(*input)
            .ok_or_else(|| crate::VmError::Runtime("truncated DCFS run length".into()))?;
        *input += 1;
        value |= usize::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
        if shift >= usize::BITS {
            return Err(crate::VmError::Runtime("DCFS run length overflow".into()));
        }
    }
}

fn record_keys_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Str(left), Value::Str(right)) => left == right,
        _ => left.as_i32() == right.as_i32(),
    }
}

fn value_ptr(value: &Value) -> Option<u32> {
    match value {
        Value::Ptr(ptr) => Some(*ptr),
        Value::Int(ptr) if *ptr >= 0 => Some(*ptr as u32),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyed_record_fetch_uses_zero_for_success_and_replaces_duplicate_keys() {
        let mut vm = Vm::new();
        let handle = vm
            .sys_record_table_open(0x100, 8)
            .expect("open record table")
            .as_i32() as u32;
        let src = vm.alloc_heap(8);
        let dst = vm.alloc_heap(8);

        vm.write_int(src, 2, 3).expect("write script index");
        vm.write_int(src + 4, 2, 0x1234)
            .expect("write function address");
        vm.sys_record_table_copy(handle, Value::Str("_Wait".into()), src)
            .expect("insert record");

        vm.write_int(src, 2, 4).expect("replace script index");
        vm.write_int(src + 4, 2, 0x5678)
            .expect("replace function address");
        vm.sys_record_table_copy(handle, Value::Str("_Wait".into()), src)
            .expect("replace record");

        let found = vm
            .sys_record_table_fetch(dst, handle, Value::Str("_Wait".into()), 0)
            .expect("fetch record");
        assert!(matches!(found, Value::Int(0)));
        assert_eq!(vm.read_int(dst, 2).expect("read script index"), 4);
        assert_eq!(
            vm.read_int(dst + 4, 2).expect("read function address"),
            0x5678
        );

        let missing = vm
            .sys_record_table_fetch(dst, handle, Value::Str("_Missing".into()), 0)
            .expect("fetch missing record");
        assert!(matches!(missing, Value::Int(value) if value != 0));
    }

    #[test]
    fn indexed_records_keep_newest_first_and_enforce_capacity() {
        let mut vm = Vm::new();
        let slot = vm.alloc_heap(4);
        assert_eq!(vm.sys_indexed_record_open(slot, 2, 8).unwrap(), 0);
        let handle = vm.read_int(slot, 2).unwrap();
        let src = vm.alloc_heap(8);
        let dst = vm.alloc_heap(8);

        for value in [10, 20, 30] {
            vm.write_int(src, 2, value).unwrap();
            vm.write_value(src + 4, 2, &Value::Str(format!("record-{value}")))
                .unwrap();
            assert_eq!(vm.sys_indexed_record_push(handle, src).unwrap(), 0);
        }

        let count = vm.alloc_heap(4);
        assert_eq!(vm.sys_indexed_record_count(count, handle).unwrap(), 0);
        assert_eq!(vm.read_int(count, 2).unwrap(), 2);
        assert_eq!(vm.sys_indexed_record_load(dst, handle, 0).unwrap(), 0);
        assert_eq!(vm.read_int(dst, 2).unwrap(), 30);
        assert_eq!(
            vm.mem_values.get(&Vm::value_key(dst + 4)),
            Some(&Value::Str("record-30".into()))
        );
        assert_eq!(vm.sys_indexed_record_load(dst, handle, 1).unwrap(), 0);
        assert_eq!(vm.read_int(dst, 2).unwrap(), 20);
        assert_eq!(vm.sys_indexed_record_load(dst, handle, 2).unwrap(), 2);
        assert_eq!(vm.sys_indexed_record_remove(handle, 0, 1), 0);
        assert_eq!(vm.sys_indexed_record_load(dst, handle, 0).unwrap(), 0);
        assert_eq!(vm.read_int(dst, 2).unwrap(), 20);
        assert_eq!(vm.sys_indexed_record_remove(handle, 1, 1), 2);
        assert_eq!(vm.sys_indexed_record_close(handle), 0);
        assert_eq!(vm.sys_indexed_record_close(handle), 1);
    }

    #[test]
    fn indexed_record_open_rejects_zero_dimensions() {
        let mut vm = Vm::new();
        assert_eq!(vm.sys_indexed_record_open(0x100, 0, 8).unwrap(), 2);
        assert_eq!(vm.sys_indexed_record_open(0x100, 8, 0).unwrap(), 2);
    }
}

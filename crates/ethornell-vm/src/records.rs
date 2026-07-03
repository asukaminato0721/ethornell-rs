use crate::{Value, Vm, VmResult};

#[derive(Debug, Clone, Copy)]
pub(crate) struct RecordTableState {
    pub(crate) slot: u32,
    pub(crate) record_size: u32,
    pub(crate) cursor: u32,
    pub(crate) capacity: u32,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct IndexedRecordState {
    pub(crate) slot: u32,
    pub(crate) capacity: u32,
    pub(crate) record_size: u32,
    pub(crate) populated: u32,
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

    pub(crate) fn write_sdc_script_records(&mut self, dst: u32) -> VmResult<i32> {
        // SDC FORMAT 1.00 is used by title._bp as a tiny script database.
        // The full encrypted SDC decoder is still being expanded; this keeps
        // the VM-visible table shape faithful enough for the script dispatcher.
        const RECORD_SIZE: usize = 124;
        let names = ["main", "MakerLogo", "_GM", "macro_story", "macro_chara"];
        let range = self.resolve_range(dst, RECORD_SIZE * names.len())?;
        self.memory[range].fill(0);
        self.clear_shadow_values(dst, RECORD_SIZE * names.len());
        for (index, name) in names.iter().enumerate() {
            self.write_c_string(dst.saturating_add((index * RECORD_SIZE) as u32), name)?;
        }
        Ok(names.len() as i32)
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
        dst: u32,
        src: u32,
    ) -> VmResult<()> {
        let handle_addr = Self::value_key(handle);
        if let Some((index, slot, record_size, capacity)) =
            self.record_tables.get_mut(&handle_addr).map(|table| {
                let index = table.cursor;
                table.cursor = table.cursor.saturating_add(1);
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
                    dst = format_args!("0x{dst:08X}"),
                    src = format_args!("0x{src:08X}"),
                    "RecordTableCopy"
                );
            }
            if slot == 1972 && record_size == 124 {
                self.store_sdc_script_name(index, src)?;
            } else if record_size > 0 {
                let table_dst = handle_addr.saturating_add(index.saturating_mul(record_size));
                self.copy_record_into_table(table_dst, src, record_size as usize)?;
            }
        } else if dst as usize + 4 <= self.memory.len() && src as usize + 4 <= self.memory.len() {
            let value = self.read_int(src, 2)?;
            self.write_int(dst, 2, value)?;
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
        let Some(table) = self.record_tables.get(&handle_addr).copied() else {
            return Ok(Value::Int(0));
        };
        if table.record_size == 0 {
            return Ok(Value::Int(0));
        }

        let Some(index) = self.find_record_index(handle_addr, table, &selector, mode)? else {
            if std::env::var_os("DEBUG").is_some() {
                tracing::debug!(
                    slot = format_args!("0x{:08X}", table.slot),
                    handle = format_args!("0x{handle:08X}"),
                    selector = ?selector,
                    mode,
                    "RecordTableFetchMiss"
                );
            }
            return Ok(Value::Int(0));
        };

        let src = handle_addr.saturating_add(index.saturating_mul(table.record_size));
        self.copy_buffer(dst, src, table.record_size as usize)?;
        if std::env::var_os("DEBUG").is_some() {
            tracing::debug!(
                index,
                slot = format_args!("0x{:08X}", table.slot),
                record_size = table.record_size,
                handle = format_args!("0x{handle:08X}"),
                src = format_args!("0x{src:08X}"),
                dst = format_args!("0x{dst:08X}"),
                selector = ?selector,
                mode,
                "RecordTableFetch"
            );
        }
        Ok(Value::Int(1))
    }

    pub(crate) fn sys_indexed_record_open(
        &mut self,
        slot: u32,
        capacity: u32,
        record_size: u32,
    ) -> VmResult<()> {
        let handle = self.alloc_heap(4);
        self.write_value(slot, 2, &Value::Ptr(handle))?;
        self.indexed_record_tables.insert(
            Self::value_key(handle),
            IndexedRecordState {
                slot,
                capacity,
                record_size,
                populated: 0,
            },
        );

        if record_size > 0 {
            self.ensure_indexed_record_globals(record_size)?;
        }

        if std::env::var_os("DEBUG").is_some() {
            tracing::info!(
                slot = format_args!("0x{slot:08X}"),
                capacity,
                record_size,
                handle = format_args!("0x{handle:08X}"),
                "IndexedRecordOpen"
            );
        }
        Ok(())
    }

    pub(crate) fn sys_indexed_record_close(&mut self, handle: u32) {
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
        }
    }

    pub(crate) fn sys_indexed_record_count(&mut self, dst: u32, handle: u32) -> VmResult<()> {
        let count = self
            .indexed_record_tables
            .get(&Self::value_key(handle))
            .map(|state| state.populated.min(state.capacity))
            .unwrap_or_default();
        self.write_int(dst, 2, count)?;
        if std::env::var_os("DEBUG").is_some() {
            tracing::info!(
                dst = format_args!("0x{dst:08X}"),
                handle = format_args!("0x{handle:08X}"),
                count,
                "IndexedRecordCount"
            );
        }
        Ok(())
    }

    pub(crate) fn sys_indexed_record_load(
        &mut self,
        dst: u32,
        handle: u32,
        index: u32,
    ) -> VmResult<()> {
        let state = self
            .indexed_record_tables
            .get(&Self::value_key(handle))
            .copied()
            .unwrap_or(IndexedRecordState {
                slot: 0,
                capacity: 0,
                record_size: 0,
                populated: 0,
            });
        let size = state.record_size as usize;
        if size > 0 {
            self.ensure_writable(dst, size);
            if Self::should_clear_indexed_record_target(dst) {
                self.clear_record_buffer(dst, size);
            }
        }

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
                record_size = state.record_size,
                capacity = state.capacity,
                msg_base = format_args!("0x{msg_base:08X}"),
                msg_valid,
                msg_count,
                "IndexedRecordLoad"
            );
        }
        Ok(())
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
        table: RecordTableState,
        selector: &Value,
        mode: i32,
    ) -> VmResult<Option<u32>> {
        if mode == 0 {
            let index = selector.as_i32();
            if index >= 0 && (index as u32) < table.cursor {
                return Ok(Some(index as u32));
            }
        }

        let selector_text = match selector {
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

    fn store_sdc_script_name(&mut self, index: u32, src: u32) -> VmResult<()> {
        let text = self.read_c_string(src).unwrap_or_default();
        let global = 0x0005_3098u32.saturating_add(index.saturating_mul(0x30));
        if !text.is_empty() {
            self.remember_script_record(global, &text)?;
            tracing::debug!(
                index,
                source = format_args!("0x{src:08X}"),
                global = format_args!("0x{global:08X}"),
                text,
                "SdcRecordStore"
            );
        }
        Ok(())
    }
}

use crate::Vm;

impl Vm {
    pub(crate) fn trace_msgwnd_loop(&self, offset: u32) {
        if std::env::var_os("TRACE_MSGWND_LOOP").is_none() {
            return;
        }
        if !matches!(offset, 0x1072 | 0x1078 | 0x1085 | 0x1095) {
            return;
        }
        let Some(program) = self
            .programs
            .get(self.current_program)
            .and_then(|program| program.script_name.as_deref())
        else {
            return;
        };
        if !program.ends_with("msgwndctrl._bp") {
            return;
        }

        let counter_addr = 0x1200_0000 | self.mem_ptr.saturating_sub(532);
        let table_ptr_addr = 0x1200_0000 | self.mem_ptr.saturating_sub(2024);
        let local_record = 0x1200_0000 | self.mem_ptr.saturating_sub(2016);
        let counter = self.read_int(counter_addr, 2).unwrap_or_default();
        let table_ptr = self.read_int(table_ptr_addr, 2).unwrap_or_default();
        let table_count = self
            .read_int(table_ptr.saturating_add(16), 2)
            .unwrap_or_default();
        let record_addr = table_ptr
            .saturating_add(20)
            .saturating_add(counter.saturating_mul(228));
        let record_words = self.debug_words(record_addr, 10);
        let local_words = self.debug_words(local_record, 10);

        tracing::warn!(
            pc = self.pc,
            offset = format_args!("0x{offset:08X}"),
            mem_ptr = format_args!("0x{:08X}", self.mem_ptr),
            counter_addr = format_args!("0x{counter_addr:08X}"),
            counter,
            table_ptr_addr = format_args!("0x{table_ptr_addr:08X}"),
            table_ptr = format_args!("0x{table_ptr:08X}"),
            table_count,
            record_addr = format_args!("0x{record_addr:08X}"),
            local_record = format_args!("0x{local_record:08X}"),
            ?record_words,
            ?local_words,
            "MsgWndLoopTrace"
        );
    }

    fn debug_words(&self, addr: u32, count: usize) -> Vec<String> {
        (0..count)
            .map(|index| {
                let ptr = addr.saturating_add((index * 4) as u32);
                match self.read_int(ptr, 2) {
                    Ok(value) => format!("+{:02X}=0x{value:08X}", index * 4),
                    Err(_) => format!("+{:02X}=<bad>", index * 4),
                }
            })
            .collect()
    }
}

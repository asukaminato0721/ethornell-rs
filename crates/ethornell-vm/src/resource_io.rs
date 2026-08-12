use super::*;

impl Vm {
    pub(crate) fn sys80_30_read_file_bytes<A>(&mut self, api: &mut A) -> VmResult<Value>
    where
        A: SysApi,
    {
        // Target sub_488750 pops file, archive/root, then destination. Its
        // sub_465AB0 helper clears four dwords before resolving and decoding
        // the resource directly into the caller's buffer.
        let file = self.pop_string_lossy()?;
        let archive = self.pop_string_lossy()?;
        let destination = self.pop_ptr()?;

        let header = self.resolve_write_range(destination, 16)?;
        self.memory[header].fill(0);
        self.clear_shadow_values(destination, 16);

        let Some((bytes, origin)) = self.load_resource_bytes_with_search(api, &archive, &file)
        else {
            tracing::info!(
                archive,
                file,
                destination = format_args!("0x{destination:08X}"),
                "Sys80_30_ReadFileBytesMiss"
            );
            return Ok(Value::Int(0));
        };
        if bytes.len() > 0x0400_0000 {
            tracing::warn!(
                archive,
                file,
                size = bytes.len(),
                "Sys80_30_ReadFileBytesRejectedOversizeResource"
            );
            return Ok(Value::Int(0));
        }

        let destination_range = self.resolve_write_range(destination, bytes.len())?;
        self.memory[destination_range].copy_from_slice(&bytes);
        self.clear_shadow_values(destination, bytes.len());
        tracing::info!(
            archive,
            file,
            destination = format_args!("0x{destination:08X}"),
            size = bytes.len(),
            ?origin,
            "Sys80_30_ReadFileBytes"
        );
        Ok(Value::Int(bytes.len() as i32))
    }
}

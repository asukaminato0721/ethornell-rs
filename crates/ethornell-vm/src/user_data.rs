use crate::{SysApi, Vm, VmError, VmResult};

pub(crate) const GLOBAL_CONFIG_SIZE: usize = 0x400;
pub(crate) const GLOBAL_USER_DATA_SIZE: usize = 0x10_0000;

const RAW_FIXED_SIZE: usize = 0x10_0424;
const SDC_HEADER_SIZE: usize = 32;
const GDB_MAGIC: &[u8; 16] = b"BURIKO GDB 3.00\0";
const SDC_MAGIC: &[u8; 16] = b"SDC FORMAT 1.00\0";

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct GlobalLoadResult {
    pub(crate) window_x: i32,
    pub(crate) window_y: i32,
    pub(crate) status: i32,
}

impl Vm {
    pub(crate) fn encode_user_data_buffer(
        &mut self,
        destination: u32,
        source: u32,
        length: i32,
    ) -> VmResult<i32> {
        let Ok(length) = usize::try_from(length) else {
            return Ok(0);
        };
        if length == 0 {
            return Ok(0);
        }
        let source = self.resolve_range(source, length)?;
        let encoded = encode_sdc(&self.memory[source], 0);
        let output = self.resolve_write_range(destination, encoded.len())?;
        self.memory[output].copy_from_slice(&encoded);
        self.clear_shadow_values(destination, encoded.len());
        Ok(encoded.len().min(i32::MAX as usize) as i32)
    }

    pub(crate) fn encode_user_data_structs(
        &mut self,
        destination: u32,
        source: u32,
        record_size: i32,
        record_count: i32,
    ) -> VmResult<i32> {
        let (Ok(record_size), Ok(record_count)) =
            (usize::try_from(record_size), usize::try_from(record_count))
        else {
            return Ok(0);
        };
        if record_size == 0 || record_count == 0 {
            return Ok(0);
        }
        let byte_len = record_size
            .checked_mul(record_count)
            .ok_or_else(|| VmError::Runtime("DCFS source size overflow".into()))?;
        let source = self.resolve_range(source, byte_len)?;
        let dcfs = encode_dcfs(&self.memory[source], record_size, record_count)?;
        let encoded = encode_sdc(&dcfs, 0);
        let output = self.resolve_write_range(destination, encoded.len())?;
        self.memory[output].copy_from_slice(&encoded);
        self.clear_shadow_values(destination, encoded.len());
        Ok(encoded.len().min(i32::MAX as usize) as i32)
    }

    pub(crate) fn allocate_global_config(&mut self, shift: i32) -> bool {
        let Ok(shift) = u32::try_from(shift) else {
            return false;
        };
        if shift > 12 {
            return false;
        }
        self.global_config = vec![0; 4096usize << shift];
        true
    }

    pub(crate) fn save_config_slot<A: SysApi>(
        &mut self,
        api: &mut A,
        slot: i32,
        label: &str,
    ) -> VmResult<bool> {
        let (encoded_label, _, _) = encoding_rs::SHIFT_JIS.encode(label);
        if encoded_label.len() >= 40 {
            return Err(VmError::Runtime(
                "Sys80:78 save label must be shorter than 40 Shift-JIS bytes".into(),
            ));
        }
        let mut bytes = vec![0u8; 64 + self.global_config.len()];
        write_native_system_time(&mut bytes[..16]);
        bytes[16..16 + encoded_label.len()].copy_from_slice(&encoded_label);
        bytes[16 + encoded_label.len()] = 0;
        bytes[64..].copy_from_slice(&self.global_config);
        if self.save_data_integrity_enabled != 0 {
            encrypt_config_slot(&mut bytes);
        }
        Ok(api.write_file_bytes(&config_slot_name(slot), &bytes))
    }

    pub(crate) fn load_config_slot<A: SysApi>(&mut self, api: &mut A, slot: i32) -> i32 {
        let preserved_len = self.global_config.len().min(1024);
        let preserved = self.global_config[..preserved_len].to_vec();
        let status = match self.read_config_slot(api, slot) {
            Ok(bytes) => {
                self.global_config.copy_from_slice(&bytes[64..]);
                0
            }
            Err(status) => status,
        };
        self.global_config[..preserved_len].copy_from_slice(&preserved);
        status
    }

    pub(crate) fn read_config_slot_header<A: SysApi>(
        &mut self,
        api: &mut A,
        slot: i32,
    ) -> Result<[u8; 64], i32> {
        let Some(bytes) = api.read_user_file_bytes(&config_slot_name(slot)) else {
            return Err(1);
        };
        if bytes.is_empty() {
            return Err(1);
        }
        if bytes.len() != 64 + self.global_config.len() {
            return Err(2);
        }
        let mut header = [0u8; 64];
        header.copy_from_slice(&bytes[..64]);
        Ok(header)
    }

    pub(crate) fn validate_config_slot<A: SysApi>(&mut self, api: &mut A, slot: i32) -> i32 {
        match self.read_config_slot(api, slot) {
            Ok(_) => 0,
            Err(status) => status,
        }
    }

    fn read_config_slot<A: SysApi>(&mut self, api: &mut A, slot: i32) -> Result<Vec<u8>, i32> {
        let Some(mut bytes) = api.read_user_file_bytes(&config_slot_name(slot)) else {
            return Err(1);
        };
        if bytes.is_empty() {
            return Err(1);
        }
        if bytes.len() != 64 + self.global_config.len() {
            return Err(2);
        }
        if self.save_data_integrity_enabled != 0 && !decrypt_and_validate_config_slot(&mut bytes) {
            return Err(3);
        }
        Ok(bytes)
    }

    pub(crate) fn load_global_user_data<A: SysApi>(
        &mut self,
        api: &mut A,
    ) -> VmResult<GlobalLoadResult> {
        let Some(encoded) = api.read_user_file_bytes("BGI.gdb") else {
            return Ok(GlobalLoadResult {
                status: 1,
                ..GlobalLoadResult::default()
            });
        };
        let raw = match decode_sdc(&encoded) {
            Ok(raw) => raw,
            Err(err) => {
                tracing::warn!(%err, "LoadGlobalUserData rejected BGI.gdb");
                return Ok(GlobalLoadResult {
                    status: 2,
                    ..GlobalLoadResult::default()
                });
            }
        };
        match self.install_global_data(&raw) {
            Ok(result) => Ok(result),
            Err(err) => {
                tracing::warn!(%err, "LoadGlobalUserData rejected decoded payload");
                Ok(GlobalLoadResult {
                    status: 2,
                    ..GlobalLoadResult::default()
                })
            }
        }
    }

    pub(crate) fn save_global_user_data<A: SysApi>(&mut self, api: &mut A) -> bool {
        // The first 1 KiB of the target's global allocation is also the BP
        // common-memory configuration block.  Keep the serialized mirror in
        // step with writes made by the script before producing BGI.gdb.
        let config_len = GLOBAL_CONFIG_SIZE
            .min(self.global_config.len())
            .min(self.memory.len());
        self.global_config[..config_len].copy_from_slice(&self.memory[..config_len]);
        let encoded = encode_sdc(&self.serialize_global_data(), 0);
        api.write_file_bytes("BGI.gdb", &encoded)
    }

    pub(crate) fn copy_to_global_data(
        &mut self,
        offset: i32,
        src: u32,
        length: i32,
    ) -> VmResult<()> {
        let range = checked_global_range(offset, length)?;
        let src_range = self.resolve_range(src, range.len())?;
        self.global_user_data[range].copy_from_slice(&self.memory[src_range]);
        Ok(())
    }

    pub(crate) fn copy_from_global_data(
        &mut self,
        dst: u32,
        offset: i32,
        length: i32,
    ) -> VmResult<()> {
        let range = checked_global_range(offset, length)?;
        let bytes = self.global_user_data[range].to_vec();
        let dst_range = self.resolve_write_range(dst, bytes.len())?;
        self.memory[dst_range].copy_from_slice(&bytes);
        self.clear_shadow_values(dst, bytes.len());
        Ok(())
    }

    fn serialize_global_data(&self) -> Vec<u8> {
        let names = self.resource_names.clone();
        let names_size = 4usize + names.iter().map(|name| sjis_len(name) + 1).sum::<usize>();
        let read_flags_size = 4usize
            + self
                .read_flags
                .iter()
                .map(|(name, flags)| sjis_len(name) + 1 + 4 + flags.bytes.len())
                .sum::<usize>();
        let mut raw = vec![0u8; RAW_FIXED_SIZE + names_size + read_flags_size];
        let raw_len = raw.len() as u32;
        raw[..16].copy_from_slice(GDB_MAGIC);
        write_u32(&mut raw, 16, raw_len);
        write_u32(&mut raw, 28, GLOBAL_CONFIG_SIZE as u32);
        let config_len = self.global_config.len().min(GLOBAL_CONFIG_SIZE);
        raw[32..32 + config_len].copy_from_slice(&self.global_config[..config_len]);
        write_u32(&mut raw, 0x420, GLOBAL_USER_DATA_SIZE as u32);
        raw[0x424..RAW_FIXED_SIZE].copy_from_slice(&self.global_user_data);

        let mut cursor = RAW_FIXED_SIZE;
        write_u32(&mut raw, cursor, names.len() as u32);
        cursor += 4;
        for name in names {
            cursor += write_sjis_c_string(&mut raw[cursor..], &name);
        }
        write_u32(&mut raw, cursor, self.read_flags.len() as u32);
        cursor += 4;
        for (name, flags) in &self.read_flags {
            cursor += write_sjis_c_string(&mut raw[cursor..], name);
            write_u32(&mut raw, cursor, flags.bit_len);
            cursor += 4;
            raw[cursor..cursor + flags.bytes.len()].copy_from_slice(&flags.bytes);
            cursor += flags.bytes.len();
        }
        raw
    }

    fn install_global_data(&mut self, raw: &[u8]) -> VmResult<GlobalLoadResult> {
        if raw.len() < RAW_FIXED_SIZE || raw.get(..16) != Some(GDB_MAGIC) {
            return Err(VmError::Runtime("invalid BURIKO GDB payload".into()));
        }
        if read_u32(raw, 16)? as usize != raw.len() {
            return Err(VmError::Runtime(
                "BURIKO GDB payload length mismatch".into(),
            ));
        }
        if read_u32(raw, 28)? as usize != GLOBAL_CONFIG_SIZE
            || read_u32(raw, 0x420)? as usize != GLOBAL_USER_DATA_SIZE
        {
            return Err(VmError::Runtime(
                "unsupported BURIKO GDB memory layout".into(),
            ));
        }
        if self.global_config.len() < GLOBAL_CONFIG_SIZE {
            self.global_config.resize(GLOBAL_CONFIG_SIZE, 0);
        }
        self.global_config[..GLOBAL_CONFIG_SIZE].copy_from_slice(&raw[32..0x420]);
        let config = self.global_config[..GLOBAL_CONFIG_SIZE].to_vec();
        let destination = self.resolve_write_range(0, config.len())?;
        self.memory[destination].copy_from_slice(&config);
        self.clear_shadow_values(0, config.len());
        self.global_user_data
            .copy_from_slice(&raw[0x424..RAW_FIXED_SIZE]);

        let mut cursor = RAW_FIXED_SIZE;
        let name_count = read_u32_at_cursor(raw, &mut cursor)? as usize;
        let mut names = Vec::with_capacity(name_count);
        for _ in 0..name_count {
            names.push(read_sjis_c_string(raw, &mut cursor)?);
        }
        self.resource_names = names;

        let flag_count = read_u32_at_cursor(raw, &mut cursor)? as usize;
        self.read_flags.clear();
        for _ in 0..flag_count {
            let name = read_sjis_c_string(raw, &mut cursor)?;
            let bit_len = read_u32_at_cursor(raw, &mut cursor)?;
            let byte_len = bit_len.div_ceil(8) as usize;
            let end = cursor
                .checked_add(byte_len)
                .filter(|end| *end <= raw.len())
                .ok_or_else(|| VmError::Runtime("truncated BURIKO GDB read flags".into()))?;
            self.read_flags.insert(
                name,
                crate::ReadFlagBits {
                    bit_len,
                    bytes: raw[cursor..end].to_vec(),
                },
            );
            cursor = end;
        }
        Ok(GlobalLoadResult {
            window_x: read_u32(raw, 20)? as i32,
            window_y: read_u32(raw, 24)? as i32,
            status: 0,
        })
    }
}

fn config_slot_name(slot: i32) -> String {
    format!("BGI{slot:04}.cad")
}

fn write_native_system_time(destination: &mut [u8]) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = now.as_secs();
    let days = (seconds / 86_400) as i64;
    let (year, month, day) = civil_from_days_for_save(days);
    let fields = [
        year as u16,
        month as u16,
        ((seconds / 86_400 + 4) % 7) as u16,
        day as u16,
        ((seconds / 3_600) % 24) as u16,
        ((seconds / 60) % 60) as u16,
        (seconds % 60) as u16,
        now.subsec_millis() as u16,
    ];
    for (index, value) in fields.into_iter().enumerate() {
        destination[index * 2..index * 2 + 2].copy_from_slice(&value.to_le_bytes());
    }
}

fn civil_from_days_for_save(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe as i32 + era as i32 * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i32::from(month <= 2);
    (year, month as u32, day as u32)
}

fn slot_random_next(state: &mut u32) -> u16 {
    let high = ((22_695_477u64 * u64::from(*state)) >> 16) as u16;
    *state = (u32::from(high) << 16)
        .wrapping_add(u32::from((20_021u32.wrapping_mul(*state)) as u16))
        .wrapping_add(1);
    high & 0x7fff
}

fn encrypt_config_slot(bytes: &mut [u8]) {
    let mut state = u32::from(u16::from_le_bytes([bytes[14], bytes[15]]));
    let mut sum = 0u8;
    let mut xor = 0u8;
    for byte in &mut bytes[64..] {
        *byte = byte.wrapping_add(slot_random_next(&mut state) as u8);
        sum = sum.wrapping_add(*byte);
        xor ^= *byte;
    }
    bytes[56] = sum;
    bytes[57] = xor;
    bytes[58] = slot_random_next(&mut state) as u8;
    bytes[59] = slot_random_next(&mut state) as u8;
    bytes[60] = 1;
}

fn decrypt_and_validate_config_slot(bytes: &mut [u8]) -> bool {
    let mut state = u32::from(u16::from_le_bytes([bytes[14], bytes[15]]));
    let mut sum = 0u8;
    let mut xor = 0u8;
    for byte in &mut bytes[64..] {
        let encrypted = *byte;
        sum = sum.wrapping_add(encrypted);
        xor ^= encrypted;
        *byte = encrypted.wrapping_sub(slot_random_next(&mut state) as u8);
    }
    bytes[56] == sum && bytes[57] == xor
}

fn encode_dcfs(raw: &[u8], record_size: usize, record_count: usize) -> VmResult<Vec<u8>> {
    let expected = record_size
        .checked_mul(record_count)
        .ok_or_else(|| VmError::Runtime("DCFS source size overflow".into()))?;
    if raw.len() != expected || record_size == 0 || record_count == 0 {
        return Err(VmError::Runtime("invalid DCFS record array".into()));
    }

    let mut encoded = Vec::with_capacity(24 + expected.saturating_mul(3) / 2);
    encoded.extend_from_slice(b"DCFS FORMAT 1.00");
    encoded.extend_from_slice(&(record_size as u32).to_le_bytes());
    encoded.extend_from_slice(&(record_count as u32).to_le_bytes());
    encoded.extend_from_slice(&raw[..record_size]);

    for record_index in 1..record_count {
        let previous = &raw[(record_index - 1) * record_size..record_index * record_size];
        let current = &raw[record_index * record_size..(record_index + 1) * record_size];
        let mut cursor = 0usize;
        let mut different = false;
        while cursor < record_size {
            let start = cursor;
            if different {
                while cursor < record_size && previous[cursor] != current[cursor] {
                    cursor += 1;
                }
            } else {
                while cursor < record_size && previous[cursor] == current[cursor] {
                    cursor += 1;
                }
            }
            write_dcfs_varint(&mut encoded, cursor - start);
            if different {
                encoded.extend_from_slice(&current[start..cursor]);
            }
            different = !different;
        }
    }
    Ok(encoded)
}

fn write_dcfs_varint(output: &mut Vec<u8>, mut value: usize) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        output.push(byte);
        if value == 0 {
            break;
        }
    }
}

fn checked_global_range(offset: i32, length: i32) -> VmResult<std::ops::Range<usize>> {
    let offset = usize::try_from(offset)
        .map_err(|_| VmError::Runtime("negative global-data offset".into()))?;
    let length = usize::try_from(length)
        .map_err(|_| VmError::Runtime("negative global-data length".into()))?;
    if length == 0 {
        return Err(VmError::Runtime("zero-length global-data copy".into()));
    }
    let end = offset
        .checked_add(length)
        .filter(|end| *end <= GLOBAL_USER_DATA_SIZE)
        .ok_or_else(|| VmError::Runtime("global-data range exceeds 1 MiB".into()))?;
    Ok(offset..end)
}

fn encode_sdc(raw: &[u8], seed: u32) -> Vec<u8> {
    let mut packed = Vec::with_capacity(raw.len() + raw.len().div_ceil(128));
    for chunk in raw.chunks(128) {
        packed.push((chunk.len() - 1) as u8);
        packed.extend_from_slice(chunk);
    }
    let mut state = seed;
    for byte in &mut packed {
        *byte = byte.wrapping_add(next_sdc_random(&mut state));
    }
    let sum = packed
        .iter()
        .fold(0u16, |sum, byte| sum.wrapping_add(u16::from(*byte)));
    let xor = packed.iter().fold(0u16, |xor, byte| xor ^ u16::from(*byte));
    let mut encoded = vec![0u8; SDC_HEADER_SIZE + packed.len()];
    encoded[..16].copy_from_slice(SDC_MAGIC);
    write_u32(&mut encoded, 16, seed);
    write_u32(&mut encoded, 20, packed.len() as u32);
    write_u32(&mut encoded, 24, raw.len() as u32);
    encoded[28..30].copy_from_slice(&sum.to_le_bytes());
    encoded[30..32].copy_from_slice(&xor.to_le_bytes());
    encoded[32..].copy_from_slice(&packed);
    encoded
}

fn decode_sdc(encoded: &[u8]) -> VmResult<Vec<u8>> {
    if encoded.len() < SDC_HEADER_SIZE || encoded.get(..16) != Some(SDC_MAGIC) {
        return Err(VmError::Runtime("invalid SDC header".into()));
    }
    let seed = read_u32(encoded, 16)?;
    let packed_len = read_u32(encoded, 20)? as usize;
    let raw_len = read_u32(encoded, 24)? as usize;
    let end = SDC_HEADER_SIZE
        .checked_add(packed_len)
        .filter(|end| *end == encoded.len())
        .ok_or_else(|| VmError::Runtime("SDC packed length mismatch".into()))?;
    let mut packed = encoded[SDC_HEADER_SIZE..end].to_vec();
    let expected_sum = u16::from_le_bytes(encoded[28..30].try_into().unwrap());
    let expected_xor = u16::from_le_bytes(encoded[30..32].try_into().unwrap());
    let sum = packed
        .iter()
        .fold(0u16, |sum, byte| sum.wrapping_add(u16::from(*byte)));
    let xor = packed.iter().fold(0u16, |xor, byte| xor ^ u16::from(*byte));
    if sum != expected_sum || xor != expected_xor {
        return Err(VmError::Runtime("SDC checksum mismatch".into()));
    }
    let mut state = seed;
    for byte in &mut packed {
        *byte = byte.wrapping_sub(next_sdc_random(&mut state));
    }
    let raw = unpack_sdc_lz(&packed)?;
    if raw.len() != raw_len {
        return Err(VmError::Runtime("SDC decoded length mismatch".into()));
    }
    Ok(raw)
}

fn unpack_sdc_lz(packed: &[u8]) -> VmResult<Vec<u8>> {
    let mut input = 0usize;
    let mut output = Vec::new();
    while input < packed.len() {
        let control = packed[input];
        input += 1;
        if control & 0x80 == 0 {
            let count = usize::from(control) + 1;
            let end = input
                .checked_add(count)
                .filter(|end| *end <= packed.len())
                .ok_or_else(|| VmError::Runtime("truncated SDC literal".into()))?;
            output.extend_from_slice(&packed[input..end]);
            input = end;
        } else {
            let low = *packed
                .get(input)
                .ok_or_else(|| VmError::Runtime("truncated SDC back-reference".into()))?;
            input += 1;
            let count = usize::from((control >> 3) & 0x0f) + 2;
            let distance = (usize::from(control & 7) << 8) + usize::from(low) + 2;
            if distance > output.len() {
                return Err(VmError::Runtime("invalid SDC back-reference".into()));
            }
            for _ in 0..count {
                output.push(output[output.len() - distance]);
            }
        }
    }
    Ok(output)
}

fn next_sdc_random(state: &mut u32) -> u8 {
    let low = *state as u16 as u32;
    let high = (*state >> 16) as u16 as u32;
    let low_product = low * 0x4e35;
    let next_high = high
        .wrapping_mul(0x4e35)
        .wrapping_add(state.wrapping_mul(0x015a))
        .wrapping_add(low_product >> 16)
        & 0xffff;
    // Match the native 32-bit add.  A carry from 0xffff + 1 belongs in the
    // high word and must not be discarded before the halves are combined.
    let next_low = (low_product & 0xffff).wrapping_add(1);
    *state = (next_high << 16).wrapping_add(next_low);
    (next_high & 0x7fff) as u8
}

fn read_u32(bytes: &[u8], offset: usize) -> VmResult<u32> {
    bytes
        .get(offset..offset + 4)
        .and_then(|bytes| bytes.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or_else(|| VmError::Runtime("truncated BURIKO GDB integer".into()))
}

fn read_u32_at_cursor(bytes: &[u8], cursor: &mut usize) -> VmResult<u32> {
    let value = read_u32(bytes, *cursor)?;
    *cursor += 4;
    Ok(value)
}

fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn sjis_len(value: &str) -> usize {
    encoding_rs::SHIFT_JIS.encode(value).0.len()
}

fn write_sjis_c_string(dst: &mut [u8], value: &str) -> usize {
    let (encoded, _, _) = encoding_rs::SHIFT_JIS.encode(value);
    let len = encoded.len();
    dst[..len].copy_from_slice(&encoded);
    dst[len] = 0;
    len + 1
}

fn read_sjis_c_string(bytes: &[u8], cursor: &mut usize) -> VmResult<String> {
    let tail = bytes
        .get(*cursor..)
        .ok_or_else(|| VmError::Runtime("truncated BURIKO GDB string".into()))?;
    let len = tail
        .iter()
        .position(|byte| *byte == 0)
        .ok_or_else(|| VmError::Runtime("unterminated BURIKO GDB string".into()))?;
    let (decoded, _, _) = encoding_rs::SHIFT_JIS.decode(&tail[..len]);
    *cursor += len + 1;
    Ok(decoded.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdc_random_preserves_low_word_carry() {
        let mut state = 0x0000_ebe3;

        assert_eq!(next_sdc_random(&mut state), 0xdd);
        assert_eq!(state, 0x18de_0000);
    }

    #[test]
    fn sdc_literal_encoder_round_trips() {
        let raw = (0..4097).map(|value| value as u8).collect::<Vec<_>>();
        for seed in [0, 1, 999] {
            assert_eq!(decode_sdc(&encode_sdc(&raw, seed)).unwrap(), raw);
        }
    }

    #[test]
    fn dcfs_struct_encoder_round_trips_through_native_decoder() {
        let mut vm = Vm::new();
        let source = 0x2000;
        let encoded = 0x4000;
        let restored = 0x8000;
        let record_size = 300usize;
        let record_count = 3usize;
        let mut records = (0..record_size * record_count)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        let (first, later) = records.split_at_mut(record_size);
        later[..180].copy_from_slice(&first[..180]);
        records[record_size * 2] = records[record_size] ^ 0xff;
        let range = vm.resolve_write_range(source, records.len()).unwrap();
        vm.memory[range].copy_from_slice(&records);

        let encoded_len = vm
            .encode_user_data_structs(encoded, source, record_size as i32, record_count as i32)
            .unwrap();
        assert!(encoded_len > 32);
        assert_eq!(
            vm.decode_sdc_struct_array(encoded, restored).unwrap(),
            record_count as i32
        );
        let restored = vm.resolve_range(restored, records.len()).unwrap();
        assert_eq!(&vm.memory[restored], records);
    }

    #[test]
    fn global_payload_round_trips_memory_names_and_flags() {
        let mut vm = Vm::new();
        vm.global_config[134] = 42;
        vm.global_user_data[123..127].copy_from_slice(&[1, 2, 3, 4]);
        vm.resource_names = vec!["BG0001".into(), "face".into()];
        vm.read_flags
            .insert("main".into(), crate::ReadFlagBits::new(17));
        vm.read_flags.get_mut("main").unwrap().set(16, true);
        let raw = vm.serialize_global_data();

        let mut restored = Vm::new();
        let result = restored.install_global_data(&raw).unwrap();
        assert_eq!(result.status, 0);
        assert_eq!(restored.memory[134], 42);
        assert_eq!(&restored.global_user_data[123..127], &[1, 2, 3, 4]);
        assert_eq!(
            restored.resource_names,
            vec!["BG0001".to_string(), "face".to_string()]
        );
        assert_eq!(
            restored.read_flags.get("main").unwrap().contains(16),
            Some(true)
        );
    }

    #[derive(Default)]
    struct SlotApi {
        files: std::collections::BTreeMap<String, Vec<u8>>,
    }

    impl SysApi for SlotApi {
        fn call_sys(&mut self, _call: &mut crate::NativeCallFrame) -> VmResult<crate::Value> {
            unreachable!("slot tests invoke only the file hooks")
        }

        fn write_file_bytes(&mut self, path: &str, bytes: &[u8]) -> bool {
            self.files.insert(path.to_owned(), bytes.to_vec());
            true
        }

        fn read_user_file_bytes(&mut self, path: &str) -> Option<Vec<u8>> {
            self.files.get(path).cloned()
        }
    }

    #[test]
    fn config_slot_prng_matches_target_first_step() {
        let mut state = 1u32;
        assert_eq!(slot_random_next(&mut state), 346);
        assert_eq!(state, 0x015A_4E36);
    }

    #[test]
    fn config_slot_save_validate_header_and_load_match_native_contract() {
        let mut vm = Vm::new();
        assert!(vm.allocate_global_config(0));
        for (index, byte) in vm.global_config.iter_mut().enumerate() {
            *byte = (index % 251) as u8;
        }
        let original = vm.global_config.clone();
        vm.save_data_integrity_enabled = 1;
        let mut api = SlotApi::default();

        assert!(vm.save_config_slot(&mut api, 7, "slot-seven").unwrap());
        let encoded = api.files.get("BGI0007.cad").unwrap();
        assert_eq!(encoded.len(), 64 + original.len());
        assert_eq!(&encoded[16..26], b"slot-seven");
        assert_eq!(encoded[26], 0);
        assert_eq!(encoded[60], 1);
        assert_ne!(&encoded[64..], original.as_slice());
        assert_eq!(vm.validate_config_slot(&mut api, 7), 0);
        assert_eq!(
            &vm.read_config_slot_header(&mut api, 7).unwrap()[16..26],
            b"slot-seven"
        );

        vm.global_config.fill(0xEE);
        assert_eq!(vm.load_config_slot(&mut api, 7), 0);
        assert!(vm.global_config[..1024].iter().all(|byte| *byte == 0xEE));
        assert_eq!(&vm.global_config[1024..], &original[1024..]);
    }

    #[test]
    fn config_slot_status_codes_distinguish_missing_size_and_integrity() {
        let mut vm = Vm::new();
        assert!(vm.allocate_global_config(0));
        vm.save_data_integrity_enabled = 1;
        let mut api = SlotApi::default();

        assert_eq!(vm.validate_config_slot(&mut api, 1), 1);
        api.files.insert("BGI0001.cad".into(), vec![0; 65]);
        assert_eq!(vm.validate_config_slot(&mut api, 1), 2);

        vm.save_config_slot(&mut api, 1, "valid").unwrap();
        api.files.get_mut("BGI0001.cad").unwrap()[64] ^= 0x80;
        assert_eq!(vm.validate_config_slot(&mut api, 1), 3);
        assert!(vm.save_config_slot(&mut api, 2, &"x".repeat(40)).is_err());
    }

    #[test]
    fn config_slot_selectors_use_the_target_vm_dispatch_path() {
        let mut vm = Vm::new();
        assert!(vm.allocate_global_config(0));
        vm.global_config.fill(0x5a);
        let mut api = SlotApi::default();

        vm.stack
            .extend([crate::Value::Int(12), crate::Value::Str("dispatch".into())]);
        assert_eq!(
            vm.try_builtin_sys_with_api(&mut api, 0x80, 0x78).unwrap(),
            Some(crate::Value::None)
        );
        assert!(api.files.contains_key("BGI0012.cad"));

        let header = 0x3200;
        vm.stack
            .extend([crate::Value::Ptr(header), crate::Value::Int(12)]);
        assert_eq!(
            vm.try_builtin_sys_with_api(&mut api, 0x80, 0x7a).unwrap(),
            Some(crate::Value::Int(0))
        );
        assert_eq!(vm.read_c_string(header + 16).unwrap(), "dispatch");

        vm.stack.push(crate::Value::Int(12));
        assert_eq!(
            vm.try_builtin_sys_with_api(&mut api, 0x80, 0x7b).unwrap(),
            Some(crate::Value::Int(0))
        );
        vm.global_config[1024..].fill(0);
        vm.stack.push(crate::Value::Int(12));
        assert_eq!(
            vm.try_builtin_sys_with_api(&mut api, 0x80, 0x79).unwrap(),
            Some(crate::Value::Int(0))
        );
        assert!(vm.global_config[1024..].iter().all(|byte| *byte == 0x5a));
        assert!(vm.stack.is_empty());
    }
}

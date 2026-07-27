/// Native handlers completed inside the VM before the host API dispatch.
///
/// These calls either read/write VM-owned memory, manipulate VM scheduler
/// state, or require typed pointer records. Keeping the list explicit lets the
/// host layer prove that every recovered native dispatch entry has one owner.
pub fn owns_before_host_dispatch(group: u8, id: u16) -> bool {
    matches!(
        (group, id),
        (
            0x80,
            0x00 | 0x01
                | 0x02
                | 0x04
                | 0x05
                | 0x06
                | 0x0C
                | 0x0D
                | 0x0F
                | 0x11
                | 0x12
                | 0x14
                | 0x16
                | 0x17
                | 0x18
                | 0x19
                | 0x1A
                | 0x20
                | 0x21
                | 0x30
                | 0x32
                | 0x38
                | 0x39
                | 0x3A
                | 0x3B
                | 0x3D
                | 0x45
                | 0x47
                | 0x48
                | 0x49
                | 0x4A
                | 0x4B
                | 0x4C
                | 0x5E
                | 0x71
                | 0x84
                | 0x85
                | 0x88
                | 0x89
                | 0x8A
                | 0x8B
                | 0x9A
                | 0x9C
                | 0xA0
                | 0xA1
                | 0xAC
                | 0xC0
                | 0xC4
                | 0xD3
                | 0xD9
                | 0xDB
                | 0xDC
                | 0xDD
        ) | (0x90, 0x14 | 0x15 | 0x29)
            | (0x91, 0x95 | 0x9B | 0x9F)
            | (0x92, 0x12)
            | (0xB0, 0x05 | 0x80 | 0xC1 | 0xC4 | 0xC7)
    )
}

#[cfg(test)]
mod tests {
    use super::owns_before_host_dispatch;

    #[test]
    fn every_vm_owned_call_is_native_registered() {
        for group in [0x80, 0x81, 0x90, 0x91, 0x92, 0xA0, 0xB0, 0xC0] {
            for id in 0..=u8::MAX {
                if owns_before_host_dispatch(group, u16::from(id)) {
                    assert!(
                        ethornell_script::native_abi::lookup(group, u16::from(id)).is_some(),
                        "VM owns unregistered call 0x{group:02X}:0x{id:02X}"
                    );
                }
            }
        }
    }
}

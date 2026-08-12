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
                | 0x07
                | 0x09
                | 0x0A
                | 0x0B
                | 0x0C
                | 0x0D
                | 0x0F
                | 0x10
                | 0x11
                | 0x12
                | 0x13
                | 0x14
                | 0x15
                | 0x16
                | 0x17
                | 0x18
                | 0x19
                | 0x1A
                | 0x1B
                | 0x1C
                | 0x1D
                | 0x20
                | 0x21
                | 0x25
                | 0x26
                | 0x30
                | 0x31
                | 0x32
                | 0x33
                | 0x34
                | 0x35
                | 0x36
                | 0x37
                | 0x38
                | 0x39
                | 0x3A
                | 0x3B
                | 0x3D
                | 0x40
                | 0x41
                | 0x44
                | 0x45
                | 0x46
                | 0x47
                | 0x48
                | 0x49
                | 0x4A
                | 0x4B
                | 0x4C
                | 0x50
                | 0x52
                | 0x58
                | 0x5A
                | 0x5C
                | 0x5E
                | 0x5F
                | 0x62
                | 0x67
                | 0x68
                | 0x6A
                | 0x6D
                | 0x70
                | 0x71
                | 0x78
                | 0x79
                | 0x7A
                | 0x7B
                | 0x80
                | 0x81
                | 0x82
                | 0x83
                | 0x84
                | 0x85
                | 0x88
                | 0x89
                | 0x8A
                | 0x8B
                | 0x90
                | 0x91
                | 0x94
                | 0x95
                | 0x96
                | 0x97
                | 0x9A
                | 0x9C
                | 0x98
                | 0x99
                | 0x9D
                | 0x9E
                | 0xA0
                | 0xA1
                | 0xA9
                | 0xAC
                | 0xB0
                | 0xB1
                | 0xB4
                | 0xB5
                | 0xB6
                | 0xC0
                | 0xC1
                | 0xC4
                | 0xC5
                | 0xCF
                | 0xD0
                | 0xD1
                | 0xD2
                | 0xD3
                | 0xD4
                | 0xDA
                | 0xD9
                | 0xDB
                | 0xDC
                | 0xDD
                | 0xDE
                | 0xE9
                | 0xEA
                | 0xF1
                | 0xF3
                | 0xF4
                | 0xF5
                | 0xF6
                | 0xF9
                | 0xFA
                | 0xFB
                | 0xFC
                | 0xFE
        ) | (
            0x81,
            0x07 | 0x08
                | 0x09
                | 0x0A
                | 0x0B
                | 0x0C
                | 0x0D
                | 0x0E
                | 0x0F
                | 0x11
                | 0x17
                | 0x18
                | 0x1D
                | 0x30
                | 0x35
                | 0x60
                | 0x62
                | 0x63
                | 0x64
                | 0x6B
                | 0x6F
                | 0xB0
                | 0xB7
                | 0xE9
                | 0xEA
        ) | (
            0x90,
            0x14 | 0x15
                | 0x16
                | 0x29
                | 0xB4
                | 0xB5
                | 0xB6
                | 0xB7
                | 0xBA
                | 0xBC
                | 0xBE
                | 0xBF
                | 0xF4
                | 0xF7
        ) | (
            0x91,
            0x03 | 0x38
                | 0x3D
                | 0x3E
                | 0x73
                | 0x78
                | 0x79
                | 0x7A
                | 0x7B
                | 0x7E
                | 0x95
                | 0x9B
                | 0x9E
                | 0x9F
                | 0xBA
                | 0xBF
                | 0xF1
                | 0xF7
        ) | (0x92, 0x12 | 0x16 | 0x17 | 0x9B | 0x9E | 0xF1 | 0xF5)
            | (0xA0, 0x86)
            | (0xB0, 0x27 | 0x80 | 0xA0 | 0xA3 | 0xC1 | 0xC4 | 0xC7)
            | (0xC0, 0x06 | 0xC2 | 0xC3 | 0xF0)
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

    #[test]
    fn vm_owned_system80_tail_calls_are_declared() {
        for id in [
            0xCF, 0xDE, 0xE9, 0xEA, 0xF1, 0xF3, 0xF4, 0xF5, 0xF6, 0xF9, 0xFA, 0xFB, 0xFC, 0xFE,
        ] {
            assert!(
                owns_before_host_dispatch(0x80, id),
                "System80:{id:02X} must be intercepted before host dispatch"
            );
        }
    }
}

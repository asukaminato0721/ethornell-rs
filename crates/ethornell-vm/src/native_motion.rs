/// Target BP-side motion-manager table recovered from `mtnmngr`.
///
/// This is not a guessed Rust animation queue. The target script indexes 64
/// fixed-size records at BP memory offset 45,840 with a stride of 3,208 bytes
/// and directly inspects DWORDs at record offsets `+0x00`, `+0x04`, and
/// `+0x10`. When the worker/provision field is already nonzero, the target
/// script uses System80:0x4C to invoke the associated callback/worker.
///
/// Meanings beyond those observed accesses remain deliberately unnamed. The
/// runtime does not instantiate this field image yet; it exists so future
/// reverse work can map target fields without recreating the former global
/// `animation_queue_remaining` approximation.
pub const BP_MOTION_TABLE_OFFSET: usize = 45_840;
pub const MOTION_SLOT_COUNT: usize = 64;
pub const MOTION_SLOT_STRIDE: usize = 3_208;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MotionManagerSlotLayout32 {
    pub state_00: u32,             // +0x00, exact meaning unrecovered
    pub state_04: u32,             // +0x04, exact meaning unrecovered
    pub unknown_08_to_0f: [u8; 8], // +0x08..+0x0f
    pub worker_or_completion: u32, // +0x10, nonzero path reaches Sys80:0x4C
    pub unknown_14_to_c87: [u8; 0xc74],
}

impl Default for MotionManagerSlotLayout32 {
    fn default() -> Self {
        Self {
            state_00: 0,
            state_04: 0,
            unknown_08_to_0f: [0; 8],
            worker_or_completion: 0,
            unknown_14_to_c87: [0; 0xc74],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    #[test]
    fn motion_slot_layout_matches_target_script_stride() {
        assert_eq!(size_of::<MotionManagerSlotLayout32>(), MOTION_SLOT_STRIDE);
        assert_eq!(offset_of!(MotionManagerSlotLayout32, state_00), 0x00);
        assert_eq!(offset_of!(MotionManagerSlotLayout32, state_04), 0x04);
        assert_eq!(
            offset_of!(MotionManagerSlotLayout32, worker_or_completion),
            0x10
        );
        assert_eq!(BP_MOTION_TABLE_OFFSET, 45_840);
        assert_eq!(MOTION_SLOT_COUNT, 64);
    }
}
